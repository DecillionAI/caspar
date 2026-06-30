use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::controllers::vm_controller::VmController;
use crate::drivers::vmm::network::vm_network::VmNetworkService;
use crate::drivers::vmm::models::vm_runtime::parse_vm_resource_limits;
use crate::drivers::vmm::globals::with_global_app;

pub(crate) struct DockerVmController {
    docker: Docker,
}

impl DockerVmController {
    pub(crate) fn new() -> Result<Self, String> {
        Docker::connect_with_local_defaults()
            .map(|docker| Self { docker })
            .map_err(|e| format!("docker client init failed: {}", e))
    }

    fn with_async<T, F>(&self, fut: F) -> Result<T, String>
    where
        F: std::future::Future<Output = Result<T, BollardError>>,
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime init failed: {}", e))?
            .block_on(fut)
            .map_err(|e| format!("docker api error: {}", e))
    }

    pub(crate) fn run_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let creature_id = packet["creatureId"]
            .as_str()
            .or_else(|| packet["userId"].as_str())
            .unwrap_or("")
            .to_string();
        let (entity_id, container_name, _standalone, vm_id) = extract_docker_identity(packet);
        let vm_cache_key = if vm_id.is_empty() {
            "main".to_string()
        } else {
            vm_id.clone()
        };
        let container_id = docker_container_id(machine_id, &entity_id, &container_name, &vm_id);
        let image_ref = packet["imageRef"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| docker_image_ref(machine_id, &entity_id));
        let limits = parse_vm_resource_limits(packet);

        // Sandbox semantics for docker (parallel to the fire runtime):
        //   - `persistent: true` (the default for sandboxes) carves a
        //     non-escapable per-VM dir under `{storage}/vms/<vm>`
        //     and bind-mounts it as the container's `/data`, so software
        //     installed and data written there survive across suspend/resume.
        //   - `forceRestart: false` makes `run_vm` idempotent: if the
        //     container exists, it's started (resumed) instead of being
        //     removed and rebuilt — that's the "wake" path.
        let persistent = packet["persistent"].as_bool().unwrap_or(true);
        let force_restart = packet["forceRestart"].as_bool().unwrap_or(false);
        let mount_dir = if persistent {
            Some(docker_session_vm_dir(&vm_cache_key)?)
        } else {
            None
        };

        if !force_restart {
            if let Some(status) = self.container_state(&container_id)? {
                if status == "running" {
                    return Ok(json!({
                        "ok": true,
                        "machineId": machine_id,
                        "vmId": vm_cache_key,
                        "containerId": container_id,
                        "runtime": "docker",
                        "status": "already-running",
                        "persistent": persistent,
                        "vmDir": mount_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                    }));
                }
                // Container exists but is stopped (suspended). Start it again
                // — its writable layer and any persistent bind mount survive.
                self.with_async(self.docker.start_container::<String>(
                    &container_id,
                    None::<StartContainerOptions<String>>,
                ))?;
                with_global_app(|app| {
                    app.tools().vmm().register_vm_context(&vm_cache_key, &creature_id, machine_id)
                });
                return Ok(json!({
                    "ok": true,
                    "machineId": machine_id,
                    "vmId": vm_cache_key,
                    "containerId": container_id,
                    "runtime": "docker",
                    "status": "resumed",
                    "persistent": persistent,
                    "vmDir": mount_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                }));
            }
        } else {
            self.stop_and_remove_if_exists(&container_id)?;
        }

        // Base env from the caller, then append the docker-host bridge gateway
        // coordinates + this container's verified identity. The container uses
        // these to open its single TCP connection back to the node and conduct
        // all host interactions over it.
        let mut env_vars = packet["env"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        env_vars.extend(gateway_container_env(&vm_cache_key));
        let env = Some(env_vars);
        let cmd = packet["command"]
            .as_str()
            .map(|command| vec!["sh".to_string(), "-lc".to_string(), command.to_string()]);

        // Container sandbox runtime. Defaults to gVisor ("runsc") so the
        // production security posture is preserved: every creature runs
        // sandboxed. Environments without gVisor installed (local dev / CI,
        // `run-nodes.sh --no-gvisor`) set `CASPAR_DOCKER_RUNTIME=runc` (or
        // `default`/empty) so containers fall back to the stock Docker runtime
        // instead of failing to start against an unregistered `runsc`.
        let runtime = match std::env::var("CASPAR_DOCKER_RUNTIME") {
            Ok(v) => {
                let v = v.trim();
                if v.is_empty()
                    || v.eq_ignore_ascii_case("default")
                    || v.eq_ignore_ascii_case("runc")
                {
                    None
                } else {
                    Some(v.to_string())
                }
            }
            Err(_) => Some("runsc".to_string()),
        };

        // Per-container disk quota via `storage_opt` only works on a backing
        // storage driver that supports it (overlay2 on XFS with pquota, or
        // btrfs/zfs). Applied by default to match production; drivers that
        // reject the option (e.g. plain overlayfs in dev/CI) opt out with
        // `CASPAR_DOCKER_DISK_QUOTA=0` (also accepts off/false/no).
        let storage_opt = match std::env::var("CASPAR_DOCKER_DISK_QUOTA") {
            Ok(v)
                if matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "off" | "false" | "no"
                ) =>
            {
                None
            }
            _ => Some(HashMap::from([(
                "size".to_string(),
                format!("{}G", limits.disk_gb),
            )])),
        };

        // run_vm is launched fire-and-forget by /programs/runEntity (the caller
        // discards this Result), so a failed create/start would otherwise be
        // completely silent and the creature merely "never serves". Emit the
        // failure into THIS vm's log stream (the same channel the launcher polls
        // for TOOL_SERVE_READY) so the real cause — e.g. a storage_opt disk quota
        // smaller than the image, an unknown runtime, or out-of-space — is
        // visible instead of an opaque "FAILED to serve".
        self.with_async(self.docker.create_container(
            Some(CreateContainerOptions {
                name: container_id.clone(),
                platform: None,
            }),
            DockerConfig {
                image: Some(image_ref),
                env,
                cmd,
                host_config: Some(HostConfig {
                    runtime,
                    network_mode: Some(VmNetworkService::gateway_network_name().to_string()),
                    // Resolve `host.docker.internal` to the node host inside the
                    // bridge network so the container can dial the gateway.
                    extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
                    memory: Some((limits.ram_mb * 1024 * 1024) as i64),
                    cpu_count: Some(limits.cpu_cores as i64),
                    storage_opt,
                    // Per-session persistent storage: the non-escapable host
                    // dir under `{storage}/vms/<vm>` is bind-mounted
                    // as `/data` in the container so the guest's data and
                    // installed software survive across suspend/resume.
                    binds: mount_dir
                        .as_ref()
                        .map(|d| vec![format!("{}:/data", d.display())]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )).map_err(|e| {
            emit_vm_log(&vm_cache_key, "runtime",
                        &format!("CONTAINER_CREATE_FAILED {}", e));
            e
        })?;

        if !packet["inputFiles"].is_null() {
            let files = parse_input_files(&packet["inputFiles"])?;
            self.upload_files(&container_id, "/app/input", &files)?;
        }

        self.with_async(
            self.docker
                .start_container::<String>(&container_id, None::<StartContainerOptions<String>>),
        ).map_err(|e| {
            emit_vm_log(&vm_cache_key, "runtime",
                        &format!("CONTAINER_START_FAILED {}", e));
            e
        })?;
        let timeout_container = container_id.clone();
        let timeout_vm = vm_cache_key.clone();
        let timeout_secs = limits.max_exec_time_secs;
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(timeout_secs));
            if let Ok(docker) = Docker::connect_with_local_defaults() {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                if let Ok(rt) = rt {
                    let _ = rt.block_on(docker.stop_container(
                        &timeout_container,
                        Some(StopContainerOptions { t: 1 }),
                    ));
                    let _ = rt.block_on(docker.remove_container(
                        &timeout_container,
                        Some(RemoveContainerOptions {
                            force: true,
                            v: true,
                            ..Default::default()
                        }),
                    ));
                }
            }
            // Commit and remove the lifecycle transaction buffer.
            with_global_app(|app| app.tools().vmm().commit_vm_trx(timeout_vm.as_str()));
            // DashMap: no lock needed — inserts/removes are concurrency-safe
            with_global_app(|app| app.tools().vmm().unregister_vm_context(timeout_vm.as_str()));
            // Drop the container→identity binding so its IP can't be reused.
            with_global_app(|app| app.tools().vmm().unregister_vm_container(timeout_container.as_str()));
        });
        let container_id_for_logs = container_id.clone();
        let vm_id_for_logs = vm_cache_key.clone();
        thread::spawn(move || {
            let docker = match Docker::connect_with_local_defaults() {
                Ok(d) => d,
                Err(_) => return,
            };
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(_) => return,
            };
            rt.block_on(async move {
                let mut stream = docker.logs(
                    &container_id_for_logs,
                    Some(LogsOptions::<String> {
                        follow: true,
                        stdout: true,
                        stderr: true,
                        // "all" (not "0"): short-lived creatures print and exit
                        // within milliseconds, before this follower attaches.
                        // tail:"0" would only stream NEW lines and miss their
                        // entire output; "all" replays the full buffered log.
                        tail: "all".to_string(),
                        ..Default::default()
                    }),
                );
                while let Some(msg) = stream.try_next().await.unwrap_or(None) {
                    match msg {
                        LogOutput::StdOut { message }
                        | LogOutput::StdErr { message }
                        | LogOutput::Console { message }
                        | LogOutput::StdIn { message } => {
                            let line = String::from_utf8_lossy(&message).to_string();
                            emit_vm_log(&vm_id_for_logs, "runtime", line.trim());
                        }
                    }
                }
            });
        });
        // DashMap: no lock needed — inserts/removes are concurrency-safe
        with_global_app(|app| app.tools().vmm().register_vm_context(&vm_cache_key, &creature_id, machine_id));
        // Bind the docker container name to this VM's authoritative identity so
        // the gateway can resolve a connection's identity from its source IP
        // (IP → container name → identity). program_id == machine_id for docker.
        with_global_app(|app| {
            app.tools().vmm().register_vm_container(
                &container_id,
                &vm_cache_key,
                &creature_id,
                machine_id,
                machine_id,
            )
        });
        // Begin a lifecycle transaction buffer for this VM execution so all
        // dbOp writes are batched and committed atomically at VM end.
        with_global_app(|app| app.tools().vmm().begin_vm_trx(&vm_cache_key));
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "vmId": vm_cache_key,
            "containerId": container_id,
            "runtime": "docker",
            "status": "running",
            "persistent": persistent,
            "vmDir": mount_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        }))
    }

    /// Turn a docker VM off. By default this is a *suspend*: the container is
    /// STOPPED but not removed, and its persistent bind mount under
    /// `{storage}/vms/<vm>` is retained, so a later `run_vm` resumes
    /// the same sandbox with installed software and saved data intact.
    /// Callers pass `purge: true` to *delete* the sandbox: stop + remove the
    /// container, then remove its persistent directory.
    pub(crate) fn terminate_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let entity_id = packet["entityId"]
            .as_str()
            .or_else(|| packet["imageName"].as_str())
            .unwrap_or("main")
            .to_string();
        let container_name = packet["containerName"]
            .as_str()
            .unwrap_or("main")
            .to_string();
        let vm_id = packet["vmId"].as_str().unwrap_or("").to_string();
        let vm_cache_key = if vm_id.is_empty() {
            "main".to_string()
        } else {
            vm_id.clone()
        };
        let purge = packet["purge"].as_bool().unwrap_or(false);
        let container_id = docker_container_id(machine_id, &entity_id, &container_name, &vm_id);

        if purge {
            self.stop_and_remove_if_exists(&container_id)?;
        } else {
            // Suspend: stop the container but DO NOT remove it. Its writable
            // layer, env, and bind mounts survive so a later `run_vm` resumes
            // exactly the same sandbox state.
            let _ = self.with_async(
                self.docker
                    .stop_container(&container_id, Some(StopContainerOptions { t: 1 })),
            );
        }

        with_global_app(|app| app.tools().vmm().commit_vm_trx(vm_cache_key.as_str()));
        with_global_app(|app| app.tools().vmm().unregister_vm_context(vm_cache_key.as_str()));
        // The container→identity binding is tied to a *running* IP; once the
        // container is stopped its IP is released, so the binding must be
        // dropped on both suspend and purge.
        with_global_app(|app| app.tools().vmm().unregister_vm_container(container_id.as_str()));

        let mut purged = false;
        if purge {
            let dir = docker_vm_dir_path(&vm_cache_key);
            if dir.exists() && docker_is_within_vms_root(&dir) {
                std::fs::remove_dir_all(&dir).map_err(|e| {
                    format!("failed to purge sandbox dir {}: {}", dir.display(), e)
                })?;
                purged = true;
            }
        }

        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "containerId": container_id,
            "runtime": "docker",
            "status": if purge { "deleted" } else { "suspended" },
            "purged": purged,
        }))
    }

    pub(crate) fn exec_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        let (entity_id, container_name, _standalone, vm_id) = extract_docker_identity(packet);
        let command = packet["command"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        if command.is_empty() {
            return Err("command is required".to_string());
        }
        let container_id = docker_container_id(machine_id, &entity_id, &container_name, &vm_id);
        // Wake-on-exec: if this sandbox is suspended (container exists but is
        // stopped) or has never been started, bring it back up on its
        // persistent volume before writing the command — so a session resumes
        // transparently on the next prompt.
        match self.container_state(&container_id)? {
            Some(s) if s == "running" => {}
            _ => {
                let _ = self.run_vm(packet);
            }
        }
        let create_res = self.with_async(self.docker.create_exec(
            &container_id,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(vec![
                    "sh".to_string(),
                    "-lc".to_string(),
                    command.to_string(),
                ]),
                ..Default::default()
            },
        ))?;
        let mut output = String::new();
        let start_res = self.with_async(self.docker.start_exec(&create_res.id, None))?;
        if let StartExecResults::Attached {
            output: mut stream, ..
        } = start_res
        {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("tokio runtime init failed: {}", e))?
                .block_on(async {
                    while let Some(msg) = stream.try_next().await? {
                        match msg {
                            LogOutput::StdOut { message }
                            | LogOutput::StdErr { message }
                            | LogOutput::Console { message }
                            | LogOutput::StdIn { message } => {
                                output.push_str(&String::from_utf8_lossy(&message));
                            }
                        }
                    }
                    Ok::<(), BollardError>(())
                })
                .map_err(|e| format!("docker exec stream error: {}", e))?;
        }
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "containerId": container_id,
            "output": output
        }))
    }

    pub(crate) fn copy_to_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        let (entity_id, container_name, _standalone, vm_id) = extract_docker_identity(packet);
        let file_name = packet["fileName"].as_str().unwrap_or("");
        let content = packet["content"].as_str().unwrap_or("");
        let target_path = packet["targetPath"].as_str().unwrap_or("/app/input");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        if file_name.is_empty() {
            return Err("fileName is required".to_string());
        }
        let container_id = docker_container_id(machine_id, &entity_id, &container_name, &vm_id);
        let mut files = HashMap::new();
        files.insert(file_name.to_string(), content.as_bytes().to_vec());
        self.upload_files(&container_id, target_path, &files)?;
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "containerId": container_id,
            "fileName": file_name
        }))
    }

    /// Resolve the docker container name that owns `ip` on any docker network.
    ///
    /// This is the authoritative, spoof-resistant way to identify which creature
    /// a gateway connection belongs to: the source IP of the connection is fixed
    /// by the docker bridge (a container cannot forge another container's bridge
    /// IP), and docker — not the container — reports which container holds it.
    pub(crate) fn find_container_name_by_ip(&self, ip: &str) -> Result<Option<String>, String> {
        if ip.is_empty() {
            return Ok(None);
        }
        let summaries = self.with_async(self.docker.list_containers(Some(
            ListContainersOptions::<String> {
                all: false,
                ..Default::default()
            },
        )))?;
        for c in summaries {
            let matched = c
                .network_settings
                .as_ref()
                .and_then(|ns| ns.networks.as_ref())
                .map(|nets| nets.values().any(|ep| ep.ip_address.as_deref() == Some(ip)))
                .unwrap_or(false);
            if matched {
                let name = c
                    .names
                    .as_ref()
                    .and_then(|n| n.first())
                    .map(|s| s.trim_start_matches('/').to_string());
                return Ok(name);
            }
        }
        Ok(None)
    }

    /// Returns the docker container state (`"running"` / `"exited"` / ...)
    /// for a given container id, or `None` if the container does not exist.
    /// Used to drive the suspend/resume + wake-on-exec semantics.
    fn container_state(&self, container_id: &str) -> Result<Option<String>, String> {
        let summaries = self.with_async(self.docker.list_containers(Some(
            ListContainersOptions::<String> {
                all: true,
                ..Default::default()
            },
        )))?;
        for c in summaries {
            let is_match = c
                .names
                .as_ref()
                .map(|names| {
                    names
                        .iter()
                        .any(|n| n.trim_start_matches('/') == container_id)
                })
                .unwrap_or(false);
            if is_match {
                return Ok(c.state.clone());
            }
        }
        Ok(None)
    }

    fn stop_and_remove_if_exists(&self, container_id: &str) -> Result<(), String> {
        let _ = self.with_async(
            self.docker
                .stop_container(container_id, Some(StopContainerOptions { t: 1 })),
        );
        self.with_async(self.docker.remove_container(
            container_id,
            Some(RemoveContainerOptions {
                force: true,
                v: true,
                ..Default::default()
            }),
        ))
        .or_else(|e| {
            if e.contains("No such container") {
                Ok(())
            } else {
                Err(e)
            }
        })
    }

    fn upload_files(
        &self,
        container_id: &str,
        target_path: &str,
        files: &HashMap<String, Vec<u8>>,
    ) -> Result<(), String> {
        let tar_bytes = build_tar(files)?;
        self.with_async(self.docker.upload_to_container(
            container_id,
            Some(UploadToContainerOptions {
                path: target_path,
                ..Default::default()
            }),
            tar_bytes.into(),
        ))
    }

    pub(crate) fn build_image(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let build_type = packet["buildType"]
            .as_str()
            .or_else(|| packet["runtime"].as_str())
            .unwrap_or("docker")
            .to_lowercase();
        let entity_id = packet["entityId"]
            .as_str()
            .or_else(|| packet["imageName"].as_str())
            .unwrap_or("main")
            .to_string();
        let image_build_path = packet["imageBuildPath"]
            .as_str()
            .or_else(|| packet["dockerfilePath"].as_str())
            .or_else(|| packet["path"].as_str())
            .unwrap_or("");
        if image_build_path.is_empty() {
            return Err("build path is required".to_string());
        }

        if build_type == "wasm" {
            let script_path = resolve_script_path(image_build_path, "build.sh")?;
            run_local_build_script(&script_path)?;
            return Ok(json!({
                "ok": true,
                "machineId": machine_id,
                "scriptPath": script_path,
                "runtime": "wasm"
            }));
        }

        if build_type == "elpify" {
            let source_path = resolve_script_path(image_build_path, "module.elpify.js")?;
            let source = std::fs::read_to_string(&source_path)
                .map_err(|e| format!("failed to read elpify source {}: {}", source_path, e))?;
            let masm = transpile_js_to_masm(&source)
                .map_err(|e| format!("failed to transpile elpify source: {}", e))?;
            let masm_path = if source_path.ends_with(".elpify.js") {
                source_path.replace(".elpify.js", ".masm")
            } else {
                let source_ref = Path::new(&source_path);
                source_ref
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("module.masm")
                    .to_string_lossy()
                    .to_string()
            };
            std::fs::write(&masm_path, masm.as_bytes())
                .map_err(|e| format!("failed to write MASM output {}: {}", masm_path, e))?;
            return Ok(json!({
                "ok": true,
                "machineId": machine_id,
                "sourcePath": source_path,
                "masmPath": masm_path,
                "runtime": "elpify"
            }));
        }

        let image_ref = packet["imageRef"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| docker_image_ref(machine_id, &entity_id));
        let context = build_context_from_path(image_build_path)?;
        let options = BuildImageOptions {
            dockerfile: "Dockerfile".to_string(),
            t: image_ref.clone(),
            rm: true,
            pull: false,
            ..Default::default()
        };

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime init failed: {}", e))?
            .block_on(async {
                let mut stream = self.docker.build_image(options, None, Some(context.into()));
                while let Some(update) = stream.try_next().await.map_err(|e| e.to_string())? {
                    if let Some(status) = update.stream.clone() {
                        emit_vm_log("main", "build", status.trim());
                    }
                    if let Some(error) = update.error {
                        emit_vm_log("main", "build", error.trim());
                        return Err(format!("docker build failed: {}", error));
                    }
                }
                Ok::<(), String>(())
            })?;

        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "imageRef": image_ref,
            "runtime": "docker"
        }))
    }
}

/// Build the environment variables that tell a docker creature how to reach the
/// docker-host bridge gateway.
///
/// Security: the container is given **no identity and no secret** — only the
/// gateway address. The node identifies which creature a connection belongs to
/// from the connection's docker-network source IP (see
/// `find_container_name_by_ip` / `IVmm::identify_container_by_ip`), which a
/// container cannot forge. `CASPAR_VM_ID` is exposed for log readability only
/// and is never trusted for auth.
///
/// IMPORTANT — bridge consistency: because identity is keyed on the source IP,
/// the advertised host MUST resolve to the node over the SAME bridge the
/// creature is attached to (`VmNetworkService::gateway_network_name`, i.e.
/// `kasper`). Its bridge-gateway IP (e.g. 172.18.0.1) is the host on that
/// network and the node already listens there (0.0.0.0:8079). The
/// `host.docker.internal` fallback is Docker's `host-gateway`, which points at
/// the *default* docker0 bridge (172.17.0.1); a `kasper` creature reaching the
/// host across that other bridge has its source IP masqueraded, so it no longer
/// matches any `kasper` endpoint and the HELLO handshake is rejected with
/// "could not identify a docker creature for source ip ..." — every tool/agent
/// then silently fails to serve. Deployments therefore set
/// `DOCKER_HOST_GATEWAY_ADVERTISE_HOST` to the kasper bridge gateway (run-nodes.sh
/// derives it from `docker network inspect kasper`); the fallback below is only
/// for the legacy single-default-bridge case.
fn gateway_container_env(vm_id: &str) -> Vec<String> {
    let host = std::env::var("DOCKER_HOST_GATEWAY_ADVERTISE_HOST")
        .unwrap_or_else(|_| "host.docker.internal".to_string());
    let port = std::env::var("DOCKER_HOST_GATEWAY_PORT").unwrap_or_else(|_| "8079".to_string());
    vec![
        format!("CASPAR_GATEWAY_HOST={}", host),
        format!("CASPAR_GATEWAY_PORT={}", port),
        format!("CASPAR_VM_ID={}", vm_id),
    ]
}

fn docker_container_id(
    machine_id: &str,
    entity_id: &str,
    container_name: &str,
    vm_id: &str,
) -> String {
    if !vm_id.is_empty() {
        return format!("{}_{}", machine_id.replace('@', "_"), vm_id);
    }
    format!(
        "{}_{}_{}",
        machine_id.replace('@', "_"),
        entity_id,
        container_name
    )
}

fn docker_image_ref(machine_id: &str, entity_id: &str) -> String {
    format!("{}/{}", machine_id.replace('@', "_"), entity_id)
}

fn extract_docker_identity(packet: &JsonValue) -> (String, String, bool, String) {
    let standalone = packet["standalone"].as_bool().unwrap_or(false)
        || packet["isStandalone"].as_bool().unwrap_or(false);
    let entity_id = packet["entityId"]
        .as_str()
        .or_else(|| packet["imageName"].as_str())
        .unwrap_or("main")
        .to_string();
    let container_name = packet["containerName"]
        .as_str()
        .unwrap_or("main")
        .to_string();
    let vm_id = packet["vmId"].as_str().unwrap_or("").to_string();
    (entity_id, container_name, standalone, vm_id)
}

fn build_tar(files: &HashMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut buf = Vec::<u8>::new();
    {
        let mut tar = TarBuilder::new(&mut buf);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).map_err(|e| e.to_string())?;
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, Cursor::new(content.as_slice()))
                .map_err(|e| e.to_string())?;
        }
        tar.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

fn parse_input_files(input_files: &JsonValue) -> Result<HashMap<String, Vec<u8>>, String> {
    if let Some(serialized) = input_files.as_str() {
        let parsed = serde_json::from_str::<JsonValue>(serialized)
            .map_err(|e| format!("inputFiles must be valid JSON: {}", e))?;
        return parse_input_files(&parsed);
    }
    let mut files = HashMap::new();
    let obj = input_files
        .as_object()
        .ok_or_else(|| "inputFiles must be an object or JSON-encoded object".to_string())?;
    for (key, value) in obj {
        let raw = value.as_str().unwrap_or("");
        files.insert(key.to_string(), raw.as_bytes().to_vec());
    }
    Ok(files)
}

// Background log-writer: build/run log capture happens *inside* tokio
// `block_on` contexts, but storage().log_vm() drives a synchronous Postgres
// (QuestDB) client whose connection is torn down on a background tokio runtime
// — dropping it from within another runtime aborts the process ("panic in a
// destructor during cleanup"). We therefore funnel every captured line to a
// single dedicated OS thread that owns no tokio context and performs the
// blocking write safely. This also decouples the (chatty) container stdout from
// the hot capture path. The router-based vmLog packet path is unsuitable here:
// it classifies by "type" (a bare {"key":"vmLog"} is dropped as Unknown) and
// the host-call path requires creature program context the follower lacks.
fn vm_log_sender() -> std::sync::mpsc::Sender<(String, String, String)> {
    static LOG_TX: std::sync::OnceLock<
        std::sync::Mutex<std::sync::mpsc::Sender<(String, String, String)>>,
    > = std::sync::OnceLock::new();
    LOG_TX
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(String, String, String)>();
            thread::spawn(move || {
                for (vm_id, log_type, text) in rx {
                    let ts = chrono::Utc::now().timestamp_millis();
                    with_global_app(|app| {
                        let _ = app.tools().storage().log_vm(&vm_id, &log_type, &text, ts);
                    });
                }
            });
            std::sync::Mutex::new(tx)
        })
        .lock()
        .unwrap()
        .clone()
}

fn emit_vm_log(vm_id: &str, log_type: &str, text: &str) {
    if vm_id.trim().is_empty() || text.trim().is_empty() {
        return;
    }
    let _ = vm_log_sender().send((vm_id.to_string(), log_type.to_string(), text.to_string()));
}

fn build_context_from_path(path: &str) -> Result<Vec<u8>, String> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(format!("dockerfile path does not exist: {}", path));
    }
    let mut buf = Vec::<u8>::new();
    {
        let mut tar = TarBuilder::new(&mut buf);
        if path_ref.is_dir() {
            tar.append_dir_all(".", path_ref)
                .map_err(|e| format!("failed to archive docker context directory: {}", e))?;
        } else {
            let parent = path_ref.parent().unwrap_or_else(|| Path::new("."));
            tar.append_dir_all(".", parent)
                .map_err(|e| format!("failed to archive docker context parent directory: {}", e))?;
        }
        tar.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
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

// ── Persistent, non-escapable per-VM storage for docker sandboxes ────────────
//
// Mirrors the fire runtime layout: `{STORAGE_ROOT_PATH}/vms/<vm>`.
// The dir is canonicalised and asserted to live inside the vms root before any
// read/write/purge — so a crafted `vmId` cannot escape the sandbox tree.

fn docker_storage_root() -> PathBuf {
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

fn docker_vms_root() -> PathBuf {
    docker_storage_root().join("vms")
}

fn docker_sanitize_component(raw: &str) -> String {
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

fn docker_vm_dir_path(vm_id: &str) -> PathBuf {
    let vm = docker_sanitize_component(if vm_id.is_empty() { "main" } else { vm_id });
    docker_vms_root().join(vm)
}

fn docker_session_vm_dir(vm_id: &str) -> Result<PathBuf, String> {
    let root = docker_vms_root();
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("failed to prepare vms root {}: {}", root.display(), e))?;
    let canon_root = std::fs::canonicalize(&root)
        .map_err(|e| format!("failed to canonicalize vms root: {}", e))?;
    let dir = docker_vm_dir_path(vm_id);
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

fn docker_is_within_vms_root(dir: &Path) -> bool {
    let root = docker_vms_root();
    let canon_root = std::fs::canonicalize(&root).unwrap_or(root);
    match std::fs::canonicalize(dir) {
        Ok(c) => c.starts_with(&canon_root) && c != canon_root,
        Err(_) => dir.starts_with(&canon_root) && dir != canon_root,
    }
}

impl VmController for DockerVmController {
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
        Err("copy_from is not implemented yet for docker runtime".to_string())
    }
}
