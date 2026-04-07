struct DockerVmController {
    docker: Docker,
}

impl DockerVmController {
    fn new() -> Result<Self, String> {
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

    fn run_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let (image_name, container_name, _standalone, vm_id) = extract_docker_identity(packet);
        let container_id = docker_container_id(machine_id, &image_name, &container_name, &vm_id);
        let image_ref = packet["imageRef"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| docker_image_ref(machine_id, &image_name));

        self.stop_and_remove_if_exists(&container_id)?;

        let env = packet["env"].as_array().map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        });
        let cmd = packet["command"]
            .as_str()
            .map(|command| vec!["sh".to_string(), "-lc".to_string(), command.to_string()]);

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
                    runtime: Some("runsc".to_string()),
                    network_mode: Some("kasper".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))?;

        if !packet["inputFiles"].is_null() {
            let files = parse_input_files(&packet["inputFiles"])?;
            self.upload_files(&container_id, "/app/input", &files)?;
        }

        self.with_async(
            self.docker
                .start_container::<String>(&container_id, None::<StartContainerOptions<String>>),
        )?;
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "containerId": container_id,
            "runtime": "docker"
        }))
    }

    fn terminate_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let image_name = packet["imageName"].as_str().unwrap_or("main").to_string();
        let container_name = packet["containerName"]
            .as_str()
            .unwrap_or("main")
            .to_string();
        let vm_id = packet["vmId"].as_str().unwrap_or("").to_string();
        let container_id = docker_container_id(machine_id, &image_name, &container_name, &vm_id);
        self.stop_and_remove_if_exists(&container_id)?;
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "containerId": container_id,
            "runtime": "docker"
        }))
    }

    fn exec_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        let (image_name, container_name, _standalone, vm_id) = extract_docker_identity(packet);
        let command = packet["command"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        if command.is_empty() {
            return Err("command is required".to_string());
        }
        let container_id = docker_container_id(machine_id, &image_name, &container_name, &vm_id);
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

    fn copy_to_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        let (image_name, container_name, _standalone, vm_id) = extract_docker_identity(packet);
        let file_name = packet["fileName"].as_str().unwrap_or("");
        let content = packet["content"].as_str().unwrap_or("");
        let target_path = packet["targetPath"].as_str().unwrap_or("/app/input");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        if file_name.is_empty() {
            return Err("fileName is required".to_string());
        }
        let container_id = docker_container_id(machine_id, &image_name, &container_name, &vm_id);
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

    fn build_image(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let image_name = packet["imageName"].as_str().unwrap_or("main").to_string();
        let dockerfile_path = packet["dockerfilePath"]
            .as_str()
            .or_else(|| packet["path"].as_str())
            .unwrap_or("");
        if dockerfile_path.is_empty() {
            return Err("dockerfilePath is required".to_string());
        }
        let image_ref = packet["imageRef"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| docker_image_ref(machine_id, &image_name));
        let context = build_context_from_path(dockerfile_path)?;
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
                    if let Some(error) = update.error {
                        return Err(format!("docker build failed: {}", error));
                    }
                }
                Ok::<(), String>(())
            })?;

        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "imageRef": image_ref
        }))
    }
}

fn docker_container_id(
    machine_id: &str,
    image_name: &str,
    container_name: &str,
    vm_id: &str,
) -> String {
    if !vm_id.is_empty() {
        return format!("{}_{}", machine_id.replace('@', "_"), vm_id);
    }
    format!(
        "{}_{}_{}",
        machine_id.replace('@', "_"),
        image_name,
        container_name
    )
}

fn docker_image_ref(machine_id: &str, image_name: &str) -> String {
    format!("{}/{}", machine_id.replace('@', "_"), image_name)
}

fn extract_docker_identity(packet: &JsonValue) -> (String, String, bool, String) {
    let standalone = packet["standalone"].as_bool().unwrap_or(false)
        || packet["isStandalone"].as_bool().unwrap_or(false);
    let image_name = packet["imageName"].as_str().unwrap_or("main").to_string();
    let container_name = packet["containerName"]
        .as_str()
        .unwrap_or("main")
        .to_string();
    let vm_id = packet["vmId"].as_str().unwrap_or("").to_string();
    (image_name, container_name, standalone, vm_id)
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

