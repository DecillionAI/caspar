use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::controllers::vm_controller::VmController;
use crate::drivers::vmm::bridge::runtime_io::{wasm_send, log};
use crate::drivers::vmm::models::vm_runtime::{parse_vm_resource_limits, VmResourceLimits};
use crate::drivers::vmm::network::vm_network::VmNetworkService;
use crate::drivers::vmm::globals::with_global_app;

pub(crate) static GLOBAL_FIRE_VMS: Lazy<Arc<Mutex<HashMap<String, FireVmProcess>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

pub(crate) struct FireVmProcess {
    pub(crate) machine_id: String,
    pub(crate) vm_id: String,
    pub(crate) requester_user_id: String,
    pub(crate) stream_store_id: String,
    pub(crate) socket_path: PathBuf,
    pub(crate) child: Child,
    pub(crate) stdin: Arc<Mutex<ChildStdin>>,
    pub(crate) output: Arc<Mutex<String>>,
    pub(crate) io_stop: Arc<AtomicBool>,
    pub(crate) stdout_thread: Option<JoinHandle<()>>,
    pub(crate) stderr_thread: Option<JoinHandle<()>>,
    /// Per-session persistent sandbox directory under `{storage}/vms/...`.
    /// Retained across suspend/resume so the session can be woken with all of
    /// its installed software and data intact; only removed on an explicit
    /// purge (delete).
    pub(crate) vm_dir: Option<PathBuf>,
    /// When true the backing `vm_dir` survives `terminate_vm` (suspend); a
    /// caller asking to delete the sandbox passes `purge: true`.
    pub(crate) persistent: bool,
}

pub(crate) struct FireVmController;

impl FireVmController {
    pub(crate) fn new() -> Result<Self, String> {
        std::fs::create_dir_all("/opt/firecracker/vms")
            .map_err(|e| format!("failed to prepare firecracker vm dir: {}", e))?;
        Ok(Self)
    }

    pub(crate) fn run_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("").trim();
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let vm_id = packet["vmId"].as_str().unwrap_or("main").trim();
        let vm_cache_key = if vm_id.is_empty() { "main" } else { vm_id };
        let requester_user_id = packet["requesterUserId"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        let requester_user_id_for_cache = requester_user_id.clone();
        let stream_store_id = packet["storeId"].as_str().unwrap_or("").trim().to_string();
        let process_key = fire_process_key(machine_id, vm_id);
        let socket_path = fire_socket_path(machine_id, vm_id);
        let limits = parse_vm_resource_limits(packet);

        // Session sandboxes are persistent by default: their disk lives under
        // `{storage}/vms` and is kept across suspend/resume cycles. A caller can
        // opt out with `persistent: false` for an ephemeral one-shot VM.
        let persistent = packet["persistent"].as_bool().unwrap_or(true);
        let force_restart = packet["forceRestart"].as_bool().unwrap_or(false);

        // Keep-alive: if this session's VM is already up, leave it running.
        // This lets davinci wake / keep a sandbox warm while it actively
        // services a session without tearing down any in-flight work.
        if !force_restart {
            let already_running = {
                let mut fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
                match fire_vms.get_mut(&process_key) {
                    Some(proc) => matches!(proc.child.try_wait(), Ok(None)),
                    None => false,
                }
            };
            if already_running {
                return Ok(json!({
                    "ok": true,
                    "runtime": "fire",
                    "machineId": machine_id,
                    "vmId": vm_id,
                    "processKey": process_key,
                    "status": "already-running",
                    "persistent": persistent,
                }));
            }
        }

        // Resolve (and create) the non-escapable per-session sandbox directory
        // before doing anything else, so a bad store/vm id is rejected early.
        let vm_dir = if persistent {
            Some(session_vm_dir(vm_id)?)
        } else {
            None
        };

        self.terminate_by_key(&process_key);
        let _ = std::fs::remove_file(&socket_path);

        let firecracker_bin = std::env::var("FIRECRACKER_BIN")
            .unwrap_or_else(|_| "/usr/local/bin/firecracker".to_string());
        let child = Command::new(&firecracker_bin)
            .arg("--api-sock")
            .arg(&socket_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to start firecracker process: {}", e))?;
        configure_firecracker_machine_limits(&socket_path, &limits)?;

        // Provision persistent storage and, when boot images are configured,
        // boot a real guest whose only visible block devices are this session's
        // own disks — the host filesystem is unreachable from inside the VM.
        if let Some(ref dir) = vm_dir {
            if let Err(err) = provision_and_boot_guest(&socket_path, dir, &limits) {
                log(format!(
                    "fire vm {}: persistent guest boot unavailable, running in scaffold mode: {}",
                    process_key, err
                ));
            }
        }

        let mut child = child;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to acquire firecracker stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to acquire firecracker stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to acquire firecracker stderr".to_string())?;

        let output = Arc::new(Mutex::new(String::new()));
        let io_stop = Arc::new(AtomicBool::new(false));

        let output_stdout = Arc::clone(&output);
        let io_stop_stdout = Arc::clone(&io_stop);
        let machine_id_stdout = machine_id.to_string();
        let vm_id_stdout = vm_id.to_string();
        let requester_stdout = requester_user_id.clone();
        let store_id_stdout = stream_store_id.clone();
        let stdout_thread = thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            while !io_stop_stdout.load(Ordering::Relaxed) {
                line.clear();
                match std::io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(mut out) = output_stdout.lock() {
                            out.push_str(&line);
                        }
                        emit_fire_output_signal(
                            &machine_id_stdout,
                            &vm_id_stdout,
                            &requester_stdout,
                            &store_id_stdout,
                            line.trim_end(),
                        );
                    }
                    Err(_) => break,
                }
            }
        });

        let output_stderr = Arc::clone(&output);
        let io_stop_stderr = Arc::clone(&io_stop);
        let machine_id_stderr = machine_id.to_string();
        let vm_id_stderr = vm_id.to_string();
        let requester_stderr = requester_user_id.clone();
        let store_id_stderr = stream_store_id.clone();
        let stderr_thread = thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stderr);
            let mut line = String::new();
            while !io_stop_stderr.load(Ordering::Relaxed) {
                line.clear();
                match std::io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(mut out) = output_stderr.lock() {
                            out.push_str(&line);
                        }
                        emit_fire_output_signal(
                            &machine_id_stderr,
                            &vm_id_stderr,
                            &requester_stderr,
                            &store_id_stderr,
                            line.trim_end(),
                        );
                    }
                    Err(_) => break,
                }
            }
        });

        let vm_dir_for_response = vm_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let mut fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
        fire_vms.insert(
            process_key.clone(),
            FireVmProcess {
                machine_id: machine_id.to_string(),
                vm_id: vm_id.to_string(),
                requester_user_id,
                stream_store_id,
                socket_path,
                child,
                stdin: Arc::new(Mutex::new(stdin)),
                output,
                io_stop,
                stdout_thread: Some(stdout_thread),
                stderr_thread: Some(stderr_thread),
                vm_dir,
                persistent,
            },
        );
        with_global_app(|app| {
            app.tools().vmm().register_vm_context(
                vm_cache_key,
                &requester_user_id_for_cache,
                machine_id,
            );
        });
        let timeout_key = process_key.clone();
        let timeout_secs = limits.max_exec_time_secs;
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(timeout_secs));
            let mut fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
            if let Some(mut proc) = fire_vms.remove(&timeout_key) {
                proc.io_stop.store(true, Ordering::Relaxed);
                let _ = proc.child.kill();
                let _ = proc.child.wait();
                if let Some(handle) = proc.stdout_thread.take() {
                    let _ = handle.join();
                }
                if let Some(handle) = proc.stderr_thread.take() {
                    let _ = handle.join();
                }
                let _ = std::fs::remove_file(proc.socket_path);
                with_global_app(|app| app.tools().vmm().unregister_vm_context(proc.vm_id.as_str()));
                let _ = wasm_send(json!({
                    "key": "vmLog",
                    "input": {
                        "vmId": proc.vm_id,
                        "logType": "runtime",
                        "text": format!("fire vm terminated due to max execution time ({}s)", timeout_secs),
                    }
                }));
            }
        });

        Ok(json!({
            "ok": true,
            "runtime": "fire",
            "machineId": machine_id,
            "vmId": vm_id,
            "processKey": process_key,
            "status": "running",
            "persistent": persistent,
            "vmDir": vm_dir_for_response,
            "resources": {
                "maxExecTimeSeconds": limits.max_exec_time_secs,
                "ramMb": limits.ram_mb,
                "diskGb": limits.disk_gb,
                "cpuCores": limits.cpu_cores
            }
        }))
    }

    /// Turn a fire VM off. By default this is a *suspend*: the guest process is
    /// stopped but the persistent sandbox directory (installed software + the
    /// mounted data disk) is retained so the session can be woken later with all
    /// of its state intact. Passing `purge: true` *deletes* the sandbox: after
    /// stopping the guest its persistent directory is removed permanently.
    pub(crate) fn terminate_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("").trim();
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let vm_id = packet["vmId"].as_str().unwrap_or("main").trim();
        let purge = packet["purge"].as_bool().unwrap_or(false);
        let vm_cache_key = if vm_id.is_empty() { "main" } else { vm_id };
        let process_key = fire_process_key(machine_id, vm_id);

        // Capture the live persistent dir (if the VM is currently running) so we
        // can purge it precisely; fall back to recomputing the deterministic
        // path so a purge still works on an already-suspended sandbox.
        let captured_dir = {
            let fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
            fire_vms.get(&process_key).and_then(|p| p.vm_dir.clone())
        };

        self.terminate_by_key(&process_key);
        with_global_app(|app| app.tools().vmm().unregister_vm_context(vm_cache_key));

        let mut purged = false;
        if purge {
            let target = captured_dir.or_else(|| vm_dir_path(vm_id));
            if let Some(dir) = target {
                if dir.exists() && is_within_vms_root(&dir) {
                    match std::fs::remove_dir_all(&dir) {
                        Ok(()) => purged = true,
                        Err(e) => {
                            return Err(format!(
                                "failed to purge sandbox dir {}: {}",
                                dir.display(),
                                e
                            ))
                        }
                    }
                }
            }
        }

        Ok(json!({
            "ok": true,
            "runtime": "fire",
            "machineId": machine_id,
            "vmId": vm_id,
            "processKey": process_key,
            "status": if purge { "deleted" } else { "suspended" },
            "purged": purged,
        }))
    }

    pub(crate) fn exec_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("").trim();
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let vm_id = packet["vmId"].as_str().unwrap_or("main").trim();
        let command = packet["command"].as_str().unwrap_or("").trim();
        if command.is_empty() {
            return Err("command is required".to_string());
        }

        let process_key = fire_process_key(machine_id, vm_id);

        // Wake-on-exec: if this session's sandbox is suspended (not currently
        // running), bring it back up on its persistent disk before running the
        // command, so a session resumes transparently on the next prompt.
        let is_running = {
            let mut fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
            match fire_vms.get_mut(&process_key) {
                Some(proc) => matches!(proc.child.try_wait(), Ok(None)),
                None => false,
            }
        };
        if !is_running {
            self.run_vm(packet)?;
        }

        let (stdin_arc, output_arc) = {
            let fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
            let proc = fire_vms
                .get(&process_key)
                .ok_or_else(|| format!("fire vm is not running: {}", process_key))?;
            (Arc::clone(&proc.stdin), Arc::clone(&proc.output))
        };

        {
            let mut stdin = stdin_arc
                .lock()
                .map_err(|_| "failed to lock fire vm stdin".to_string())?;
            std::io::Write::write_all(&mut *stdin, command.as_bytes())
                .map_err(|e| format!("failed to write command to fire vm: {}", e))?;
            std::io::Write::write_all(&mut *stdin, b"\n")
                .map_err(|e| format!("failed to write command newline to fire vm: {}", e))?;
            std::io::Write::flush(&mut *stdin)
                .map_err(|e| format!("failed to flush fire vm stdin: {}", e))?;
        }

        thread::sleep(Duration::from_millis(100));
        let output = output_arc
            .lock()
            .map_err(|_| "failed to lock fire vm output buffer".to_string())?
            .clone();

        Ok(json!({
            "ok": true,
            "runtime": "fire",
            "machineId": machine_id,
            "vmId": vm_id,
            "output": output,
        }))
    }

    pub(crate) fn copy_to_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("").trim();
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let file_name = packet["fileName"].as_str().unwrap_or("").trim();
        if file_name.is_empty() {
            return Err("fileName is required".to_string());
        }
        let target_path = packet["targetPath"].as_str().unwrap_or("/tmp").trim();
        let content = packet["content"].as_str().unwrap_or("");
        let escaped_content = content.replace('\\', "\\\\").replace('\'', "'\"'\"'");
        let copy_cmd = format!(
            "mkdir -p '{}' && printf '%s' '{}' > '{}/{}'",
            target_path, escaped_content, target_path, file_name
        );
        let mut copy_packet = packet.clone();
        copy_packet["command"] = JsonValue::String(copy_cmd);
        copy_packet["vmId"] =
            JsonValue::String(packet["vmId"].as_str().unwrap_or("main").to_string());
        self.exec_vm(&copy_packet)?;
        Ok(json!({
            "ok": true,
            "runtime": "fire",
            "machineId": machine_id,
            "fileName": file_name,
        }))
    }

    pub(crate) fn build_image(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("").trim();
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let vm_id = packet["vmId"].as_str().unwrap_or("main");
        let _ = wasm_send(json!({
            "key": "vmLog",
            "input": {
                "vmId": vm_id,
                "logType": "build",
                "text": "fire vm build image request accepted",
            }
        }));
        Ok(json!({
            "ok": true,
            "runtime": "fire",
            "machineId": machine_id,
        }))
    }

    fn terminate_by_key(&self, process_key: &str) {
        let mut fire_vms = GLOBAL_FIRE_VMS.lock().unwrap();
        if let Some(mut proc) = fire_vms.remove(process_key) {
            proc.io_stop.store(true, Ordering::Relaxed);
            let _ = proc.child.kill();
            let _ = proc.child.wait();
            if let Some(handle) = proc.stdout_thread.take() {
                let _ = handle.join();
            }
            if let Some(handle) = proc.stderr_thread.take() {
                let _ = handle.join();
            }
            let _ = std::fs::remove_file(proc.socket_path);
            let _machine_id = proc.machine_id;
            let vm_id = proc.vm_id;
            let _requester_user_id = proc.requester_user_id;
            let _stream_store_id = proc.stream_store_id;
            with_global_app(|app| app.tools().vmm().unregister_vm_context(vm_id.as_str()));
        }
    }
}

fn configure_firecracker_machine_limits(
    socket_path: &PathBuf,
    limits: &VmResourceLimits,
) -> Result<(), String> {
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if !socket_path.exists() {
        return Err(format!(
            "firecracker socket did not become available: {}",
            socket_path.display()
        ));
    }

    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)
        .map_err(|e| format!("failed to connect firecracker socket: {}", e))?;
    let body = json!({
        "vcpu_count": limits.cpu_cores,
        "mem_size_mib": limits.ram_mb,
        "track_dirty_pages": false
    })
    .to_string();
    let req = format!(
        "PUT /machine-config HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("failed to write firecracker machine-config: {}", e))?;
    stream
        .flush()
        .map_err(|e| format!("failed to flush firecracker machine-config: {}", e))?;

    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    if response.starts_with("HTTP/1.1 204") || response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!(
            "firecracker machine-config rejected response={}",
            response.lines().next().unwrap_or("")
        ))
    }
}

fn fire_process_key(machine_id: &str, vm_id: &str) -> String {
    format!("{}_{}", machine_id.replace('@', "_"), vm_id)
}

fn fire_socket_path(machine_id: &str, vm_id: &str) -> PathBuf {
    PathBuf::from(VmNetworkService::firecracker_socket(
        &machine_id.replace('@', "_"),
        vm_id,
    ))
}

fn emit_fire_output_signal(
    creature_id: &str,
    vm_id: &str,
    requester_user_id: &str,
    store_id: &str,
    output_line: &str,
) {
    if requester_user_id.is_empty() || store_id.is_empty() || output_line.is_empty() {
        return;
    }
    let payload = json!({
        "event": "fireVmOutput",
        "creatureId": creature_id,
        "vmId": vm_id,
        "requesterUserId": requester_user_id,
        "output": output_line,
    })
    .to_string();
    let signal_packet = json!({
        "key": "signal",
        "input": {
            "machineId": creature_id,
            "creatureId": creature_id,
            "storeId": store_id,
            "userId": requester_user_id,
            "type": "fire.vm.output",
            "temp": true,
            "data": payload,
        }
    });
    let _ = wasm_send(signal_packet);
    let vm_log_packet = json!({
        "key": "vmLog",
        "input": {
            "vmId": vm_id,
            "logType": "runtime",
            "text": output_line,
        }
    });
    let _ = wasm_send(vm_log_packet);
}

// ── Persistent, non-escapable session storage ────────────────────────────────

/// Root of Caspar's data storage folder, as configured for the node
/// (`STORAGE_ROOT_PATH`). Falls back to the in-container default and finally to
/// a local dev path.
fn fire_storage_root() -> PathBuf {
    if let Ok(p) = std::env::var("STORAGE_ROOT_PATH") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let in_container = PathBuf::from("/app/data/storage");
    if in_container.exists() {
        return in_container;
    }
    PathBuf::from("/tmp/caspar/storage")
}

/// The `vms` directory inside the storage folder that holds every session's
/// persistent sandbox. A store/space can hold unlimited fire VM instances here.
fn fire_vms_root() -> PathBuf {
    fire_storage_root().join("vms")
}

/// Reduce a caller-supplied id to a single safe path component. Anything that is
/// not `[A-Za-z0-9_-]` (including `/`, `.` and `..`) is replaced, so a store or
/// vm id can never be used to traverse out of the sandbox tree.
fn sanitize_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

/// Deterministic on-disk path for a sandbox (no side effects). A VM is keyed
/// solely by its globally-unique `vm_id` — the same id the rest of the vmm
/// module uses — so its persistent disk lives at `{storage}/vms/<vm>`.
fn vm_dir_path(vm_id: &str) -> Option<PathBuf> {
    let vm = sanitize_component(if vm_id.is_empty() { "main" } else { vm_id });
    Some(fire_vms_root().join(vm))
}

/// Resolve (and create) the per-VM sandbox directory under
/// `{storage}/vms/<vm>`, guaranteeing — by canonicalising the result and
/// asserting containment — that it can never resolve outside the vms root.
/// This is the persistent, non-escapable storage for the VM.
fn session_vm_dir(vm_id: &str) -> Result<PathBuf, String> {
    let root = fire_vms_root();
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("failed to prepare vms root {}: {}", root.display(), e))?;
    let canon_root = std::fs::canonicalize(&root)
        .map_err(|e| format!("failed to canonicalize vms root: {}", e))?;
    let dir = vm_dir_path(vm_id)
        .ok_or_else(|| "failed to resolve sandbox dir".to_string())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create session vm dir {}: {}", dir.display(), e))?;
    let canon_dir = std::fs::canonicalize(&dir)
        .map_err(|e| format!("failed to canonicalize session vm dir: {}", e))?;
    if !canon_dir.starts_with(&canon_root) || canon_dir == canon_root {
        return Err(format!(
            "refusing to use vm dir outside sandbox root: {}",
            canon_dir.display()
        ));
    }
    Ok(canon_dir)
}

/// Guard used before a destructive purge: the directory must sit strictly inside
/// the vms root (never the root itself or anything above it).
fn is_within_vms_root(dir: &Path) -> bool {
    let root = fire_vms_root();
    let canon_root = std::fs::canonicalize(&root).unwrap_or(root);
    match std::fs::canonicalize(dir) {
        Ok(c) => c.starts_with(&canon_root) && c != canon_root,
        Err(_) => dir.starts_with(&canon_root) && dir != canon_root,
    }
}

/// Create a sparse, ext4-formatted backing disk if it does not already exist.
/// Existing disks are left untouched so data persists across suspend/resume.
fn ensure_persistent_disk(path: &Path, disk_gb: u64) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let bytes = disk_gb
        .max(1)
        .saturating_mul(1024)
        .saturating_mul(1024)
        .saturating_mul(1024);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("failed to create disk image {}: {}", path.display(), e))?;
    file.set_len(bytes)
        .map_err(|e| format!("failed to size disk image {}: {}", path.display(), e))?;
    drop(file);
    // Best-effort filesystem so the guest can mount the data disk. If mkfs is
    // unavailable (e.g. CI without e2fsprogs) the raw backing file is still
    // provisioned and persisted; the guest can format it on first boot.
    let _ = Command::new("mkfs.ext4")
        .arg("-F")
        .arg("-q")
        .arg(path)
        .status();
    Ok(())
}

/// Configured base boot images, if both a kernel and a base rootfs exist on
/// disk. When absent, the controller runs in scaffold mode (no real guest boot)
/// but still provisions and persists the sandbox directory.
fn fire_boot_images() -> Option<(PathBuf, PathBuf)> {
    let kernel = PathBuf::from(std::env::var("FIRECRACKER_KERNEL_IMAGE").ok()?);
    let rootfs = PathBuf::from(std::env::var("FIRECRACKER_ROOTFS_IMAGE").ok()?);
    if kernel.exists() && rootfs.exists() {
        Some((kernel, rootfs))
    } else {
        None
    }
}

/// Minimal HTTP/1.1-over-unix-socket call to the Firecracker API.
fn fc_api(socket_path: &Path, method: &str, url_path: &str, body: &str) -> Result<(), String> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)
        .map_err(|e| format!("failed to connect firecracker socket: {}", e))?;
    let req = format!(
        "{} {} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        method,
        url_path,
        body.len(),
        body
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("firecracker {} {} write failed: {}", method, url_path, e))?;
    stream
        .flush()
        .map_err(|e| format!("firecracker {} {} flush failed: {}", method, url_path, e))?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    let status = response.lines().next().unwrap_or("");
    if status.contains(" 204") || status.contains(" 200") {
        Ok(())
    } else {
        Err(format!(
            "firecracker {} {} rejected: {}",
            method, url_path, status
        ))
    }
}

/// Provision the session's persistent disks and, when boot images are
/// configured, boot a real Firecracker guest whose only block devices are this
/// session's own rootfs + data disk. Because the guest sees nothing of the host
/// filesystem, the sandbox is non-escapable by construction.
fn provision_and_boot_guest(
    socket_path: &Path,
    vm_dir: &Path,
    limits: &VmResourceLimits,
) -> Result<(), String> {
    // Always provision the persistent data disk so the session's mounted path
    // exists and survives suspend/resume — even in scaffold mode.
    let data_disk = vm_dir.join("data.ext4");
    ensure_persistent_disk(&data_disk, limits.disk_gb)?;

    let (kernel, base_rootfs) = match fire_boot_images() {
        Some(pair) => pair,
        None => {
            return Err(
                "FIRECRACKER_KERNEL_IMAGE / FIRECRACKER_ROOTFS_IMAGE not configured".to_string(),
            )
        }
    };

    // Per-session writable rootfs: copy the base image once so software the
    // session installs persists across suspend/resume. Kept inside the sandbox.
    let session_rootfs = vm_dir.join("rootfs.ext4");
    if !session_rootfs.exists() {
        std::fs::copy(&base_rootfs, &session_rootfs)
            .map_err(|e| format!("failed to materialise session rootfs: {}", e))?;
    }

    // 1) Kernel boot source. The guest init is expected to mount the persistent
    //    data disk (second drive, /dev/vdb) at the sandbox mount path.
    let boot_args = std::env::var("FIRECRACKER_BOOT_ARGS")
        .unwrap_or_else(|_| "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw".to_string());
    let boot_body = json!({
        "kernel_image_path": kernel.display().to_string(),
        "boot_args": boot_args,
    })
    .to_string();
    fc_api(socket_path, "PUT", "/boot-source", &boot_body)?;

    // 2) Root device — per-session, read-write, persistent.
    let rootfs_body = json!({
        "drive_id": "rootfs",
        "path_on_host": session_rootfs.display().to_string(),
        "is_root_device": true,
        "is_read_only": false,
    })
    .to_string();
    fc_api(socket_path, "PUT", "/drives/rootfs", &rootfs_body)?;

    // 3) Persistent data disk — the non-escapable mounted sandbox path.
    let data_body = json!({
        "drive_id": "data",
        "path_on_host": data_disk.display().to_string(),
        "is_root_device": false,
        "is_read_only": false,
    })
    .to_string();
    fc_api(socket_path, "PUT", "/drives/data", &data_body)?;

    // 4) Start the guest.
    fc_api(
        socket_path,
        "PUT",
        "/actions",
        "{\"action_type\":\"InstanceStart\"}",
    )?;
    Ok(())
}

impl VmController for FireVmController {
    fn build_image(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.build_image(packet)
    }

    fn create(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.run_vm(packet)
    }

    fn starts(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.run_vm(packet)
    }

    fn stop(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.terminate_vm(packet)
    }

    fn resume(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.run_vm(packet)
    }

    fn pause(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.terminate_vm(packet)
    }

    fn exec(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.exec_vm(packet)
    }

    fn copy_to(packet: &JsonValue) -> Result<JsonValue, String> {
        let controller = Self::new()?;
        controller.copy_to_vm(packet)
    }

    fn copy_from(packet: &JsonValue) -> Result<JsonValue, String> {
        let _ = packet;
        Err("copy_from is not implemented yet for fire runtime".to_string())
    }
}
