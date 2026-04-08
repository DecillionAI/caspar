fn run_docker_db_op(machine_id: &str, input: &JsonValue) -> Result<String, String> {
    let op = input["op"].as_str().unwrap_or("");
    let key = input["key"].as_str().unwrap_or("");
    let namespaced_key = format!("{}::{}", machine_id, key);
    let db = GLOBAL_DB.lock().unwrap();
    match op {
        "put" => {
            let val = input["val"].as_str().unwrap_or("");
            db.put(namespaced_key.as_bytes(), val.as_bytes())
                .map_err(|e| format!("db put failed: {}", e))?;
            Ok("{}".to_string())
        }
        "get" => {
            let val = db
                .get(namespaced_key.as_bytes())
                .map_err(|e| format!("db get failed: {}", e))?;
            let val_str = val
                .as_ref()
                .and_then(|v| str::from_utf8(v).ok())
                .unwrap_or("")
                .to_string();
            Ok(json!({"data": val_str}).to_string())
        }
        "del" => {
            db.delete(namespaced_key.as_bytes())
                .map_err(|e| format!("db delete failed: {}", e))?;
            Ok("{}".to_string())
        }
        "getByPrefix" => {
            let prefix = input["prefix"].as_str().unwrap_or("");
            let namespaced_prefix = format!("{}::{}", machine_id, prefix);
            let mut vals = Vec::<String>::new();
            for item in db.prefix_iterator(namespaced_prefix.as_bytes()) {
                let (_, val) = item.map_err(|e| format!("db prefix iteration failed: {}", e))?;
                vals.push(String::from_utf8_lossy(&val).to_string());
            }
            Ok(json!({"data": vals}).to_string())
        }
        _ => Err("unsupported db op".to_string()),
    }
}

fn perform_http_request(input: &JsonValue) -> Result<String, String> {
    let mut url = input["url"].as_str().unwrap_or("").to_string();
    if url.is_empty() {
        return Err("url is required".to_string());
    }

    let mut method = input["method"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_uppercase();
    if method.is_empty() {
        if let Some((prefixed_method, rest_url)) = url.split_once('|') {
            method = prefixed_method.trim().to_uppercase();
            url = rest_url.to_string();
        } else {
            method = "POST".to_string();
        }
    }

    let http_method =
        Method::from_bytes(method.as_bytes()).map_err(|e| format!("invalid http method: {}", e))?;

    let mut request = Client::new().request(http_method, url);

    match &input["headers"] {
        JsonValue::Object(headers_obj) => {
            for (k, v) in headers_obj {
                if let Some(value) = v.as_str() {
                    request = request.header(k, value);
                } else {
                    request = request.header(k, v.to_string());
                }
            }
        }
        JsonValue::String(headers_raw) => {
            if !headers_raw.trim().is_empty() {
                let parsed_headers: JsonValue = serde_json::from_str(headers_raw)
                    .map_err(|e| format!("invalid headers json: {}", e))?;
                if let Some(headers_obj) = parsed_headers.as_object() {
                    for (k, v) in headers_obj {
                        if let Some(value) = v.as_str() {
                            request = request.header(k, value);
                        } else {
                            request = request.header(k, v.to_string());
                        }
                    }
                } else {
                    return Err("headers must be a JSON object".to_string());
                }
            }
        }
        JsonValue::Null => {}
        _ => return Err("headers must be a JSON object or stringified JSON object".to_string()),
    }

    if let Some(body) = input["body"].as_str() {
        request = request.body(body.to_string());
    } else if !input["body"].is_null() {
        request = request.body(input["body"].to_string());
    }

    let response = request
        .send()
        .map_err(|e| format!("http request failed: {}", e))?;
    let bytes = response
        .bytes()
        .map_err(|e| format!("failed to read response body: {}", e))?;
    Ok(BASE64_STANDARD.encode(bytes))
}

fn handle_unified_host_call(packet: &JsonValue) -> String {
    let machine_id = packet["machineId"].as_str().unwrap_or("");
    let op = packet["op"]
        .as_str()
        .or_else(|| packet["key"].as_str())
        .unwrap_or("");
    let input = if packet["input"].is_null() {
        JsonValue::Null
    } else {
        packet["input"].clone()
    };
    if machine_id.is_empty() {
        return json!({"ok": false, "error": "machineId is required"}).to_string();
    }
    match op {
        "dbOp" => match run_docker_db_op(machine_id, &input) {
            Ok(res) => res,
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        },
        "runVm" => match with_docker_controller(|controller| controller.run_vm(&input)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        },
        "terminateVm" => match with_docker_controller(|controller| controller.terminate_vm(&input))
        {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        },
        "execVm" | "execDocker" => {
            match with_docker_controller(|controller| controller.exec_vm(&input)) {
                Ok(res) => res.to_string(),
                Err(err) => json!({"ok": false, "error": err}).to_string(),
            }
        }
        "copyToVm" | "copyToDocker" => {
            match with_docker_controller(|controller| controller.copy_to_vm(&input)) {
                Ok(res) => res.to_string(),
                Err(err) => json!({"ok": false, "error": err}).to_string(),
            }
        }
        "buildVmImage" | "buildDockerImage" => {
            match with_docker_controller(|controller| controller.build_image(&input)) {
                Ok(res) => res.to_string(),
                Err(err) => json!({"ok": false, "error": err}).to_string(),
            }
        }
        "httpPost" | "httpRequest" => match perform_http_request(&input) {
            Ok(res) => res,
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        },
        "elpifyProof" | "verifyProgramExecution" => {
            let masm_path = input["masmPath"].as_str().unwrap_or("").to_string();
            let inputs = parse_u64_array_field(&input, "inputs");
            let outputs = parse_u64_array_field(&input, "outputs");
            let proof_bytes = parse_u8_array_field(&input, "proof");

            match verify_program_execution_from_packet(&masm_path, &inputs, &outputs, &proof_bytes)
            {
                Ok(security) => json!({"ok": true, "security": security}).to_string(),
                Err(err) => json!({"ok": false, "error": err}).to_string(),
            }
        }
        _ => {
            let packet = json!({
                "key": op,
                "input": input
            });
            wasm_send(packet)
        }
    }
}

fn with_docker_controller<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&DockerVmController) -> Result<T, String>,
{
    let controller = DockerVmController::new()?;
    f(&controller)
}
