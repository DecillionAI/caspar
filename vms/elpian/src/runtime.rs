//! Elpian task execution — runs an Elpian AST program to completion,
//! servicing host calls through the VMM dispatch channel.

use std::time::{Duration, Instant};

use elpian_vm::api as elpian_api;
use serde_json::{json, Value as JsonValue};

use caspar_vm_sdk::host::{host, log, set_log_vm_context};
use caspar_vm_sdk::VmResourceLimits;

fn dispatch(packet: &JsonValue) -> String {
    match host() {
        Some(h) => h.dispatch(packet),
        None => json!({"ok": false, "error": "caspar vm host is not initialised"}).to_string(),
    }
}

pub(crate) fn execute_elpian_task(
    machine_id: &str,
    vm_id: String,
    ast_path: String,
    input_raw: String,
    limits: VmResourceLimits,
) -> Result<(), String> {
    set_log_vm_context(&vm_id);
    let ast_source = std::fs::read_to_string(&ast_path)
        .map_err(|e| format!("failed to read elpian AST file {}: {}", ast_path, e))?;

    if !elpian_api::create_vm_from_ast(machine_id.to_string(), ast_source) {
        return Err("failed to create elpian VM from AST".to_string());
    }

    let input_json: JsonValue = serde_json::from_str(&input_raw).unwrap_or_else(|_| json!({}));
    let payload = if input_json.get("data").is_some() {
        input_json["data"].clone()
    } else {
        input_json
    };

    let started_at = Instant::now();
    let mut result = elpian_api::execute_vm_func_with_input(
        machine_id.to_string(),
        "main".to_string(),
        payload.to_string(),
        0,
    );
    if let Some(bytes) = elpian_api::usage(machine_id).map(|u| u.memory_bytes) {
        if bytes > (limits.ram_mb * 1024 * 1024) {
            let _ = elpian_api::destroy_vm(machine_id.to_string());
            return Err(format!(
                "elpian vm exceeded memory limit: used={} bytes limit={} bytes",
                bytes,
                limits.ram_mb * 1024 * 1024
            ));
        }
    }

    while result.has_host_call {
        if started_at.elapsed() > Duration::from_secs(limits.max_exec_time_secs) {
            let _ = elpian_api::destroy_vm(machine_id.to_string());
            return Err(format!(
                "elpian vm exceeded max execution time: {} seconds",
                limits.max_exec_time_secs
            ));
        }
        let call_data: JsonValue = serde_json::from_str(&result.host_call_data)
            .map_err(|e| format!("invalid elpian host call payload: {}", e))?;
        let host_res = json!({"value": dispatch(&call_data)}).to_string();
        result = elpian_api::continue_execution(machine_id.to_string(), host_res);
        if let Some(bytes) = elpian_api::usage(machine_id).map(|u| u.memory_bytes) {
            if bytes > (limits.ram_mb * 1024 * 1024) {
                let _ = elpian_api::destroy_vm(machine_id.to_string());
                return Err(format!(
                    "elpian vm exceeded memory limit: used={} bytes limit={} bytes",
                    bytes,
                    limits.ram_mb * 1024 * 1024
                ));
            }
        }
    }

    log(format!(
        "elpian vm executed machine={} ast={} result={}",
        machine_id, ast_path, result.result_value
    ));
    let _ = elpian_api::destroy_vm(machine_id.to_string());
    Ok(())
}
