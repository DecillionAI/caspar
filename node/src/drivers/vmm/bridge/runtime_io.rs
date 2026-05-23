use crate::drivers::vmm::prelude::*;

thread_local! {
    static LOG_VM_CONTEXT: std::cell::RefCell<String> = std::cell::RefCell::new("main".to_string());
}

pub(crate) fn set_log_vm_context(vm_id: &str) {
    let next = if vm_id.trim().is_empty() {
        "main".to_string()
    } else {
        vm_id.trim().to_string()
    };
    LOG_VM_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = next;
    });
}

fn current_log_vm_context() -> String {
    LOG_VM_CONTEXT.with(|ctx| ctx.borrow().clone())
}

pub(crate) fn log_vm(text: String, vm_id: String, log_type: &str) {
    let j = json!({
        "key": "vmLog",
        "input": {
            "text": text,
            "data": text,
            "vmId": vm_id,
            "logType": log_type
        }
    });
    wasm_send(j);
}

pub(crate) fn wasm_send(data: JsonValue) -> std::string::String {
    crate::drivers::vmm::dispatch_packet(&data)
}

pub(crate) fn log(text: String) {
    let vm_id = current_log_vm_context();
    log_vm(text, vm_id, "runtime");
}
