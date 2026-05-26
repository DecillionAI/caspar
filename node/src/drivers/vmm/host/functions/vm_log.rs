use crate::drivers::vmm::bridge::runtime_io::log_vm;
use serde_json::{json, Value as JsonValue};

pub(crate) fn host_fn_vm_log(input: &JsonValue) -> String {
    let text = input["text"].as_str().unwrap_or("");
    let vm_id = input["vmId"].as_str().unwrap_or("");
    let log_type = input["logType"].as_str().unwrap_or("runtime");
    log_vm(text.to_string(), vm_id.to_string(), log_type);
    json!({"ok": true}).to_string()
}
