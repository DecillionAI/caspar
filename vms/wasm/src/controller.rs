//! The wasm VM controller — VmPlugin implementation over the WasmEdge
//! managed runtime.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value as JsonValue};

use caspar_vm_sdk::host::{host, log, set_log_vm_context};
use caspar_vm_sdk::util::{emit_vm_error, panic_message, parse_vm_resource_limits};
use caspar_vm_sdk::{VmPlugin, VmPluginMeta};

use crate::runtime::{global_managed_vms, terminate_managed_vm, ManagedVmHandle, WasmMac};

pub struct WasmVmController {
    meta: VmPluginMeta,
}

impl WasmVmController {
    pub fn new(meta: VmPluginMeta) -> Self {
        Self { meta }
    }
}

impl VmPlugin for WasmVmController {
    fn meta(&self) -> &VmPluginMeta {
        &self.meta
    }

    /// Spawn a managed wasm VM on its own thread. Every panic and every
    /// wasmedge failure is contained inside that thread, surfaced as a
    /// vmOutput error packet, and the VM slot is always released at the end.
    fn run_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let ast_path = packet["astPath"].as_str().unwrap_or("").to_string();
        let input = packet["input"].as_str().unwrap_or("{}").to_string();
        let machine_id = packet["machineId"].as_str().unwrap_or("").to_string();
        let vm_id = packet["vmId"].as_str().unwrap_or("main").to_string();
        let limits = parse_vm_resource_limits(packet);

        let spawn_machine = machine_id.clone();
        let spawn_vm = vm_id.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                set_log_vm_context(&vm_id);
                let inp1 = input.clone();
                let input_json: JsonValue =
                    serde_json::from_str(&inp1).unwrap_or_else(|_| json!({}));
                let store_id = input_json
                    .get("store")
                    .and_then(|x| x.get("id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();

                let mut rt = WasmMac::new_vm(
                    machine_id.clone(),
                    vm_id.clone(),
                    store_id,
                    ast_path.clone(),
                    limits.ram_mb,
                    Box::new(|packet: JsonValue| match host() {
                        Some(h) => h.dispatch(&packet),
                        None => json!({"ok": false, "error": "caspar vm host is not initialised"})
                            .to_string(),
                    }),
                );
                {
                    let mut map = global_managed_vms().lock().unwrap();
                    map.insert(
                        machine_id.clone(),
                        ManagedVmHandle {
                            stop: rt.stop_flag(),
                            running: rt.running_flag(),
                        },
                    );
                }
                let stop_flag = rt.stop_flag();
                let timeout_machine = machine_id.clone();
                let timeout_vm = vm_id.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(limits.max_exec_time_secs));
                    if !stop_flag.load(Ordering::Relaxed) {
                        stop_flag.store(true, Ordering::Relaxed);
                        log(format!(
                            "wasm vm timeout reached: machine={} vm={} limit={}s",
                            timeout_machine, timeout_vm, limits.max_exec_time_secs
                        ));
                    }
                });
                let exec_res = rt.execute_on_update(inp1);
                rt.finalize();
                exec_res
            }));

            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    log(format!(
                        "wasm vm error: machine={} vm={} err={}",
                        spawn_machine, spawn_vm, err
                    ));
                    emit_vm_error(&spawn_machine, &spawn_vm, "wasm", &err);
                }
                Err(panic_payload) => {
                    let msg = panic_message(&panic_payload);
                    log(format!(
                        "wasm vm panicked: machine={} vm={} panic={}",
                        spawn_machine, spawn_vm, msg
                    ));
                    emit_vm_error(
                        &spawn_machine,
                        &spawn_vm,
                        "wasm",
                        &format!("panic: {}", msg),
                    );
                }
            }

            let mut map = global_managed_vms().lock().unwrap();
            map.remove(&spawn_machine);
        });

        Ok(json!({
            "ok": true,
            "runtime": "wasm",
            "machineId": packet["machineId"].as_str().unwrap_or(""),
            "vmId": packet["vmId"].as_str().unwrap_or("main"),
        }))
    }

    fn terminate_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        terminate_managed_vm(machine_id);
        Ok(json!({"ok": true, "runtime": "wasm", "machineId": machine_id}))
    }

    fn exec_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let ast_path = packet["astPath"].as_str().unwrap_or("").to_string();
        if ast_path.is_empty() {
            return Err("astPath is required".to_string());
        }
        Ok(json!({"ok": true, "runtime": "wasm", "astPath": ast_path}))
    }

    /// Wasm "image build": run the entity's `build.sh` script (which compiles
    /// the creature sources into `module.wasm`).
    fn build_image(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let build_path = packet["imageBuildPath"]
            .as_str()
            .or_else(|| packet["dockerfilePath"].as_str())
            .or_else(|| packet["path"].as_str())
            .unwrap_or("");
        if build_path.is_empty() {
            return Err("build path is required".to_string());
        }
        let script_path = resolve_script_path(build_path, "build.sh")?;
        run_local_build_script(&script_path)?;
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "scriptPath": script_path,
            "runtime": "wasm"
        }))
    }
}

fn resolve_script_path(path: &str, default_script_name: &str) -> Result<String, String> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(format!("build path does not exist: {}", path));
    }
    if path_ref.is_file() {
        return Ok(path.to_string());
    }
    let script_path = path_ref.join(default_script_name);
    if !script_path.exists() {
        return Err(format!(
            "required build script not found at {}",
            script_path.to_string_lossy()
        ));
    }
    Ok(script_path.to_string_lossy().to_string())
}

fn run_local_build_script(script_path: &str) -> Result<(), String> {
    let script_ref = Path::new(script_path);
    let script_name = script_ref
        .file_name()
        .ok_or_else(|| format!("invalid build script path: {}", script_path))?
        .to_string_lossy()
        .to_string();
    let cwd = script_ref
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let output = Command::new("bash")
        .arg(script_name)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to execute wasm build script {}: {}", script_path, e))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "wasm build script failed (status={}): {}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}
