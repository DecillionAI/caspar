fn main() {
    let receiver_handler = thread::spawn(|| {
        let context = zmq::Context::new();
        let responder = Arc::new(Mutex::new(context.socket(zmq::REP).unwrap()));
        {
            let res_lock = responder.lock().unwrap();
            assert!(res_lock.bind("tcp://*:5556").is_ok());
        }
        let mut msg = zmq::Message::new();
        loop {
            let mut response_payload = String::new();
            {
                let res_lock = responder.lock().unwrap();
                res_lock.recv(&mut msg, 0).unwrap();
            }
            let data = msg.as_str().unwrap();
            println!("recevied {data}");
            let packet: JsonValue = serde_json::from_str(data).unwrap();
            if packet["type"] == "runVm" {
                let runtime = packet["runtime"].as_str().unwrap_or("").to_lowercase();
                if runtime == "docker" {
                    response_payload =
                        match with_docker_controller(|controller| controller.run_vm(&packet)) {
                            Ok(res) => res.to_string(),
                            Err(err) => json!({"ok": false, "error": err}).to_string(),
                        };
                } else if runtime == "fire" {
                    response_payload =
                        match with_fire_controller(|controller| controller.run_vm(&packet)) {
                            Ok(res) => res.to_string(),
                            Err(err) => json!({"ok": false, "error": err}).to_string(),
                        };
                } else {
                    let ast_path = packet["astPath"].as_str().unwrap().to_string();
                    let input = packet["input"].as_str().unwrap().to_string();
                    let machine_id = packet["machineId"].as_str().unwrap().to_string();
                    let vm_id = packet["vmId"].as_str().unwrap_or("main").to_string();
                    let runtime = detect_vm_runtime(&packet, &ast_path);
                    if runtime == VmRuntime::Elpify {
                        let vm_handle = {
                            let mut map = GLOBAL_ELPIFY_VMS.lock().unwrap();
                            Arc::clone(map.entry(machine_id.clone()).or_insert_with(|| {
                                Arc::new(ElpifyManagedVm::new(machine_id.clone()))
                            }))
                        };
                        if let Err(e) = vm_handle.enqueue(ElpifyTask {
                            masm_path: ast_path.clone(),
                            input_raw: input.clone(),
                            vm_id: vm_id.clone(),
                        }) {
                            log(format!("failed to schedule elpify task: {}", e));
                        }
                    } else if runtime == VmRuntime::Elpian {
                        if let Err(e) = execute_elpian_task(
                            &machine_id,
                            vm_id.clone(),
                            ast_path.clone(),
                            input.clone(),
                        )
                        {
                            log(format!(
                                "elpian task failed for machine {}: {}",
                                machine_id, e
                            ));
                        }
                    } else if runtime == VmRuntime::Fire {
                        let fire_packet = json!({
                            "machineId": machine_id,
                            "vmId": packet["vmId"].as_str().unwrap_or("main"),
                        });
                        response_payload = match with_fire_controller(|controller| controller.run_vm(&fire_packet)) {
                            Ok(res) => res.to_string(),
                            Err(err) => json!({"ok": false, "error": err}).to_string(),
                        };
                    } else {
                        thread::spawn(move || {
                            set_log_vm_context(&vm_id);
                            let inp1 = input.clone();
                            let input_json: JsonValue = serde_json::from_str(&inp1).unwrap();
                            let store_id = input_json["store"].as_object().unwrap()["id"]
                                .as_str()
                                .unwrap()
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
                    }
                }
            } else if packet["type"] == "terminateVm" {
                let runtime = packet["runtime"].as_str().unwrap_or("").to_lowercase();
                if runtime == "docker" {
                    response_payload =
                        match with_docker_controller(|controller| controller.terminate_vm(&packet))
                        {
                            Ok(res) => res.to_string(),
                            Err(err) => json!({"ok": false, "error": err}).to_string(),
                        };
                } else if runtime == "fire" {
                    response_payload =
                        match with_fire_controller(|controller| controller.terminate_vm(&packet))
                        {
                            Ok(res) => res.to_string(),
                            Err(err) => json!({"ok": false, "error": err}).to_string(),
                        };
                } else {
                    let machine_id = packet["machineId"].as_str().unwrap().to_string();
                    terminate_managed_vm(&machine_id);
                }
            } else if packet["type"] == "execVm" || packet["type"] == "execDocker" {
                let runtime = packet["runtime"].as_str().unwrap_or("").to_lowercase();
                response_payload = if runtime == "fire" {
                    match with_fire_controller(|controller| controller.exec_vm(&packet)) {
                        Ok(res) => res.to_string(),
                        Err(err) => json!({"ok": false, "error": err}).to_string(),
                    }
                } else {
                    match with_docker_controller(|controller| controller.exec_vm(&packet)) {
                        Ok(res) => res.to_string(),
                        Err(err) => json!({"ok": false, "error": err}).to_string(),
                    }
                };
            } else if packet["type"] == "copyToVm" || packet["type"] == "copyToDocker" {
                let runtime = packet["runtime"].as_str().unwrap_or("").to_lowercase();
                response_payload = if runtime == "fire" {
                    match with_fire_controller(|controller| controller.copy_to_vm(&packet)) {
                        Ok(res) => res.to_string(),
                        Err(err) => json!({"ok": false, "error": err}).to_string(),
                    }
                } else {
                    match with_docker_controller(|controller| controller.copy_to_vm(&packet)) {
                        Ok(res) => res.to_string(),
                        Err(err) => json!({"ok": false, "error": err}).to_string(),
                    }
                };
            } else if packet["type"] == "buildVmImage" || packet["type"] == "buildDockerImage" {
                let runtime = packet["runtime"].as_str().unwrap_or("").to_lowercase();
                response_payload = if runtime == "fire" {
                    match with_fire_controller(|controller| controller.build_image(&packet)) {
                        Ok(res) => res.to_string(),
                        Err(err) => json!({"ok": false, "error": err}).to_string(),
                    }
                } else {
                    match with_docker_controller(|controller| controller.build_image(&packet)) {
                        Ok(res) => res.to_string(),
                        Err(err) => json!({"ok": false, "error": err}).to_string(),
                    }
                };
            } else if packet["type"] == "hostCall" {
                response_payload = handle_unified_host_call(&packet);
            } else if packet["type"] == "verifyProgramExecution" || packet["type"] == "elpifyProof" {
                let masm_path = packet["masmPath"].as_str().unwrap_or("").to_string();
                let inputs = parse_u64_array_field(&packet, "inputs");
                let outputs = parse_u64_array_field(&packet, "outputs");
                let proof_bytes = parse_u8_array_field(&packet, "proof");

                let verification_res = verify_program_execution_from_packet(
                    &masm_path,
                    &inputs,
                    &outputs,
                    &proof_bytes,
                );
                response_payload = match verification_res {
                    Ok(security) => json!({
                        "ok": true,
                        "security": security,
                    })
                    .to_string(),
                    Err(err) => json!({
                        "ok": false,
                        "error": err,
                    })
                    .to_string(),
                };
            } else if packet["type"] == "apiResponse" {
                let request_id = packet["requestId"].as_i64().unwrap();
                RESP_MAP.lock().unwrap().insert(
                    request_id,
                    packet["data"].as_str().unwrap().to_string(),
                    Duration::from_secs(180),
                );
                let mut tgm_lock = TRIGGER_MAP.lock().unwrap();
                let t_item = tgm_lock.get(&request_id);
                if !t_item.is_none() {
                    t_item.unwrap().notify_one();
                }
            }
            {
                let res_lock = responder.lock().unwrap();
                res_lock.send(response_payload, 0).unwrap();
            }
        }
    });
    let chan = GLOBAL_REQ_CHAN.clone();
    let sender_handler = thread::spawn(move || {
        println!("Connecting to host platform server...\n");
        let context = zmq::Context::new();
        let requester = context.socket(zmq::REQ).unwrap();
        assert!(requester.connect("tcp://localhost:5555").is_ok());
        let mut msg = zmq::Message::new();
        loop {
            let packet = chan.pop();
            requester.send(&packet, 0).unwrap();
            requester.recv(&mut msg, 0).unwrap();
        }
    });
    // On-chain execution pipeline is removed. Appengine now only serves runtime VM execution.
    receiver_handler.join().unwrap();
    sender_handler.join().unwrap();
}
