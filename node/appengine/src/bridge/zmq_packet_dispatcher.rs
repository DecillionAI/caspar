fn dispatch_zmq_packet(packet: &JsonValue) -> String {
    let env = ZmqPacketEnvelope::from_packet(packet);
    match env.packet_type {
        ZmqPacketType::RunVm => dispatch_run_vm_packet(packet, &env),
        ZmqPacketType::TerminateVm => dispatch_terminate_vm_packet(packet, &env),
        ZmqPacketType::ExecVm => dispatch_exec_vm_packet(packet, &env),
        ZmqPacketType::CopyToVm => dispatch_copy_to_vm_packet(packet, &env),
        ZmqPacketType::BuildVmImage => dispatch_build_vm_image_packet(packet, &env),
        ZmqPacketType::HostCall => handle_unified_host_call(packet),
        ZmqPacketType::VerifyProgramExecution => dispatch_verify_program_packet(packet),
        ZmqPacketType::ApiResponse => dispatch_api_response_packet(packet),
        ZmqPacketType::BridgeApi => match handle_bridge_api(packet) {
            Ok(payload) => payload.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        },
        ZmqPacketType::Unknown => json!({
            "ok": false,
            "error": format!("unsupported packet type: {}", packet["type"].as_str().unwrap_or(""))
        })
        .to_string(),
    }
}

fn dispatch_run_vm_packet(packet: &JsonValue, env: &ZmqPacketEnvelope) -> String {
    if env.runtime == "docker" {
        return match with_docker_controller(|controller| controller.run_vm(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }
    if env.runtime == "fire" {
        return match with_fire_controller(|controller| controller.run_vm(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }

    let ast_path = packet["astPath"].as_str().unwrap_or("").to_string();
    let input = packet["input"].as_str().unwrap_or("{}").to_string();
    let machine_id = env.machine_id.clone();
    let vm_id = env.vm_id.clone();
    let runtime = detect_vm_runtime(packet, &ast_path);

    if runtime == VmRuntime::Elpify {
        let vm_handle = {
            let mut map = GLOBAL_ELPIFY_VMS.lock().unwrap();
            Arc::clone(
                map.entry(machine_id.clone())
                    .or_insert_with(|| Arc::new(ElpifyManagedVm::new(machine_id.clone()))),
            )
        };
        if let Err(e) = vm_handle.enqueue(ElpifyTask {
            masm_path: ast_path,
            input_raw: input,
            vm_id,
        }) {
            return json!({"ok": false, "error": format!("failed to schedule elpify task: {}", e)})
                .to_string();
        }
        return json!({"ok": true, "runtime": "elpify", "machineId": machine_id}).to_string();
    }

    if runtime == VmRuntime::Elpian {
        return match execute_elpian_task(&machine_id, vm_id, ast_path, input) {
            Ok(()) => json!({"ok": true, "runtime": "elpian", "machineId": machine_id}).to_string(),
            Err(e) => json!({"ok": false, "error": format!("elpian task failed for machine {}: {}", machine_id, e)}).to_string(),
        };
    }

    if runtime == VmRuntime::Fire {
        let fire_packet = json!({
            "machineId": machine_id,
            "vmId": packet["vmId"].as_str().unwrap_or("main"),
        });
        return match with_fire_controller(|controller| controller.run_vm(&fire_packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }

    thread::spawn(move || {
        set_log_vm_context(&vm_id);
        let inp1 = input.clone();
        let input_json: JsonValue = serde_json::from_str(&inp1).unwrap_or_else(|_| json!({}));
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
            Box::new(wasm_send),
        );
        {
            let mut map = GLOBAL_MANAGED_VMS.lock().unwrap();
            map.insert(
                machine_id.clone(),
                ManagedVmHandle {
                    stop: Arc::clone(&rt.stop_),
                    running: Arc::clone(&rt.running_),
                },
            );
        }
        rt.execute_on_update(inp1);
        rt.finalize();
        let mut map = GLOBAL_MANAGED_VMS.lock().unwrap();
        map.remove(&machine_id);
    });

    json!({"ok": true, "runtime": "wasm", "machineId": env.machine_id, "vmId": env.vm_id})
        .to_string()
}

fn dispatch_terminate_vm_packet(packet: &JsonValue, env: &ZmqPacketEnvelope) -> String {
    if env.runtime == "docker" {
        return match with_docker_controller(|controller| controller.terminate_vm(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }
    if env.runtime == "fire" {
        return match with_fire_controller(|controller| controller.terminate_vm(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }
    terminate_managed_vm(&env.machine_id);
    json!({"ok": true, "machineId": env.machine_id}).to_string()
}

fn dispatch_exec_vm_packet(packet: &JsonValue, env: &ZmqPacketEnvelope) -> String {
    if env.runtime == "fire" {
        return match with_fire_controller(|controller| controller.exec_vm(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }
    match with_docker_controller(|controller| controller.exec_vm(packet)) {
        Ok(res) => res.to_string(),
        Err(err) => json!({"ok": false, "error": err}).to_string(),
    }
}

fn dispatch_copy_to_vm_packet(packet: &JsonValue, env: &ZmqPacketEnvelope) -> String {
    if env.runtime == "fire" {
        return match with_fire_controller(|controller| controller.copy_to_vm(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }
    match with_docker_controller(|controller| controller.copy_to_vm(packet)) {
        Ok(res) => res.to_string(),
        Err(err) => json!({"ok": false, "error": err}).to_string(),
    }
}

fn dispatch_build_vm_image_packet(packet: &JsonValue, env: &ZmqPacketEnvelope) -> String {
    if env.runtime == "fire" {
        return match with_fire_controller(|controller| controller.build_image(packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }
    match with_docker_controller(|controller| controller.build_image(packet)) {
        Ok(res) => res.to_string(),
        Err(err) => json!({"ok": false, "error": err}).to_string(),
    }
}

fn dispatch_verify_program_packet(packet: &JsonValue) -> String {
    let masm_path = packet["masmPath"].as_str().unwrap_or("").to_string();
    let inputs = parse_u64_array_field(packet, "inputs");
    let outputs = parse_u64_array_field(packet, "outputs");
    let proof_bytes = parse_u8_array_field(packet, "proof");

    match verify_program_execution_from_packet(&masm_path, &inputs, &outputs, &proof_bytes) {
        Ok(security) => json!({"ok": true, "security": security}).to_string(),
        Err(err) => json!({"ok": false, "error": err}).to_string(),
    }
}

fn dispatch_api_response_packet(packet: &JsonValue) -> String {
    let request_id = packet["requestId"].as_i64().unwrap_or(-1);
    if request_id >= 0 {
        RESP_MAP.lock().unwrap().insert(
            request_id,
            packet["data"].as_str().unwrap_or("").to_string(),
            Duration::from_secs(180),
        );
        let mut tgm_lock = TRIGGER_MAP.lock().unwrap();
        let t_item = tgm_lock.get(&request_id);
        if let Some(cv) = t_item {
            cv.notify_one();
        }
    }
    json!({"ok": true}).to_string()
}
