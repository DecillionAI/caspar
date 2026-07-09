use crate::drivers::vmm::prelude::*;

/// Verify a program execution proof by routing to whichever registered VM
/// runtime provides program verification (a provable runtime plugin).
pub(crate) fn host_fn_verify_program(input: &JsonValue) -> String {
    match caspar_vm_sdk::registry::verifier_plugin() {
        Some(plugin) => match plugin.verify_program_execution(input) {
            Ok(res) => res.to_string(),
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        },
        None => json!({
            "ok": false,
            "error": "no registered VM runtime provides program execution verification"
        })
        .to_string(),
    }
}
