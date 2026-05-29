use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::bridge::vm_packet_schema::{VmPacketKind, VmPacketContext};
use crate::drivers::vmm::bridge::gateway_control_api::handle_gateway_control_api;
use crate::drivers::vmm::bridge::runtime_io::{wasm_send, set_log_vm_context, log};
use crate::drivers::vmm::models::vm_runtime::{
    VmRuntime, ElpifyTask, ElpifyManagedVm, WasmMac, ManagedVmHandle,
    GLOBAL_MANAGED_VMS, GLOBAL_ELPIFY_VMS,
    detect_vm_runtime, parse_vm_resource_limits, terminate_managed_vm,
    execute_elpian_task, verify_program_execution_from_packet,
    parse_u64_array_field, parse_u8_array_field,
};
use crate::drivers::vmm::host::vm_host_functions::{handle_unified_host_call, with_docker_controller, with_fire_controller};

/// Extract a human-readable message from a `catch_unwind` payload.
pub(crate) fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

/// Publish a uniform error packet for any VM runtime failure so the host
/// observer sees the failure instead of a silent drop.
pub(crate) fn emit_vm_error(machine_id: &str, vm_id: &str, runtime: &str, err: &str) {
    let payload = json!({
        "key": "vmOutput",
        "input": {
            "text": err,
            "data": err,
            "vmId": vm_id,
            "machineId": machine_id,
            "logType": "error",
            "runtime": runtime,
        }
    });
    wasm_send(payload);
}

pub fn route_vm_packet(packet: &JsonValue) -> String {
    let owned = packet.clone();
    let dispatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let env = VmPacketContext::from_packet(&owned);
        match env.packet_type {
            VmPacketKind::RunVm => dispatch_run_vm_packet(&owned, &env),
            VmPacketKind::TerminateVm => dispatch_terminate_vm_packet(&owned, &env),
            VmPacketKind::ExecVm => dispatch_exec_vm_packet(&owned, &env),
            VmPacketKind::CopyToVm => dispatch_copy_to_vm_packet(&owned, &env),
            VmPacketKind::BuildVmImage => dispatch_build_vm_image_packet(&owned, &env),
            VmPacketKind::HostCall => handle_unified_host_call(&owned),
            VmPacketKind::VerifyProgramExecution => dispatch_verify_program_packet(&owned),
            VmPacketKind::ApiResponse => {
                json!({"ok": true, "skipped": "in-process mode"}).to_string()
            }
            VmPacketKind::BridgeApi => match handle_gateway_control_api(&owned) {
                Ok(payload) => payload.to_string(),
                Err(err) => json!({"ok": false, "error": err}).to_string(),
            },
            VmPacketKind::Unknown => json!({
                "ok": false,
                "error": format!(
                    "unsupported packet type: {}",
                    owned["type"].as_str().unwrap_or("")
                )
            })
            .to_string(),
        }
    }));
    match dispatch {
        Ok(s) => s,
        Err(payload) => {
            let msg = panic_message(&payload);
            log(format!("vmm dispatcher panic contained: {}", msg));
            json!({"ok": false, "error": format!("vmm panic: {}", msg)}).to_string()
        }
    }
}

fn dispatch_run_vm_packet(packet: &JsonValue, env: &VmPacketContext) -> String {
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
    let limits = parse_vm_resource_limits(packet);

    if runtime == VmRuntime::Elpify {
        // Elpify programs run to completion and produce a STARK proof.
        // There's nothing to "leave running", so the host fn returns
        // {outputs, proof} synchronously instead of enqueuing into the
        // async ElpifyManagedVm worker. The caller (a wasm creature,
        // typically the elpify-chain miniapp) can then pipe the proof
        // straight into elpifyProof / submitTrx for validator consensus.
        let _ = vm_id;
        let _ = limits;
        if ast_path.is_empty() {
            return json!({"ok": false, "runtime": "elpify", "error": "astPath (MASM file) is required"}).to_string();
        }
        // Two ways to pass public inputs:
        //   1. `inputs` array directly on the packet (preferred for
        //      host-fn callers).
        //   2. `input` is a JSON string of `{"inputs":[...]}` (legacy
        //      shape used by the wasm-runtime signal envelope).
        let inputs = if !packet["inputs"].is_null() {
            parse_u64_array_field(packet, "inputs")
        } else {
            match serde_json::from_str::<JsonValue>(&input) {
                Ok(parsed) => parse_u64_array_field(&parsed, "inputs"),
                Err(_) => Vec::new(),
            }
        };
        return match elpify_lang::execute_masm_file_with_proof(&ast_path, &inputs) {
            Ok(artifacts) => {
                // Encode the (tens-of-KB) STARK proof as base64 rather than a
                // JSON number array. wasm creatures round-trip this value
                // several times (persist, broadcast, re-read); a number array
                // costs seconds per pass under TinyGo's reflection-based JSON,
                // while a base64 string is a single scalar. `parse_u8_array_field`
                // (used by elpifyProof / verifyProgramExecution) decodes both.
                use base64::Engine;
                let proof_b64 = base64::engine::general_purpose::STANDARD
                    .encode(&artifacts.proof_bytes);
                json!({
                    "ok": true,
                    "runtime": "elpify",
                    "machineId": machine_id,
                    "masmPath": ast_path,
                    "inputs": inputs,
                    "outputs": artifacts.stack_outputs,
                    "proof": proof_b64,
                })
                .to_string()
            }
            Err(err) => json!({
                "ok": false,
                "runtime": "elpify",
                "machineId": machine_id,
                "error": err.to_string(),
            })
            .to_string(),
        };
    }

    if runtime == VmRuntime::Elpian {
        let limits = parse_vm_resource_limits(packet);
        let machine_for_task = machine_id.clone();
        let vm_for_task = vm_id.clone();
        let ast_for_task = ast_path.clone();
        let input_for_task = input.clone();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute_elpian_task(
                &machine_for_task,
                vm_for_task,
                ast_for_task,
                input_for_task,
                limits.clone(),
            )
        }));
        return match res {
            Ok(Ok(())) => {
                json!({"ok": true, "runtime": "elpian", "machineId": machine_id}).to_string()
            }
            Ok(Err(e)) => {
                emit_vm_error(&machine_id, &vm_id, "elpian", &e);
                json!({
                    "ok": false,
                    "error": format!("elpian task failed for machine {}: {}", machine_id, e)
                })
                .to_string()
            }
            Err(payload) => {
                let msg = panic_message(&payload);
                emit_vm_error(&machine_id, &vm_id, "elpian", &format!("panic: {}", msg));
                json!({"ok": false, "error": format!("elpian panic: {}", msg)}).to_string()
            }
        };
    }

    if runtime == VmRuntime::Fire {
        let fire_packet = json!({
            "machineId": machine_id,
            "vmId": packet["vmId"].as_str().unwrap_or("main"),
            "resources": packet["resources"].clone(),
        });
        return match with_fire_controller(|controller| controller.run_vm(&fire_packet)) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
    }

    let spawn_machine = machine_id.clone();
    let spawn_vm = vm_id.clone();
    thread::spawn(move || {
        // Contain every panic and every wasmedge failure inside this thread.
        // A panic here used to either crash the node (when bubbled through a
        // C++ frame as SIGABRT) or leave the VM map containing a half-running
        // entry. Capture the panic, surface it as a vmOutput error packet,
        // and always release the VM slot at the end.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                limits.ram_mb,
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
            let stop_flag = Arc::clone(&rt.stop_);
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

        let mut map = GLOBAL_MANAGED_VMS.lock().unwrap();
        map.remove(&spawn_machine);
    });

    json!({"ok": true, "runtime": "wasm", "machineId": env.machine_id, "vmId": env.vm_id})
        .to_string()
}

fn dispatch_terminate_vm_packet(packet: &JsonValue, env: &VmPacketContext) -> String {
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

fn dispatch_exec_vm_packet(packet: &JsonValue, env: &VmPacketContext) -> String {
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

fn dispatch_copy_to_vm_packet(packet: &JsonValue, env: &VmPacketContext) -> String {
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

fn dispatch_build_vm_image_packet(packet: &JsonValue, env: &VmPacketContext) -> String {
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

