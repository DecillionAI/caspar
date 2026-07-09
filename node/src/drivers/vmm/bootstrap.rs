//! Embedded bootstrap helpers for the VMM runtime.

use crate::drivers::vmm::prelude::*;

use caspar_vm_sdk::registry as vm_registry;

/// Start bootstrap services for the in-process VMM runtime.
///
/// The legacy request/response ZMQ bridge is removed in monolithic mode,
/// so startup is a no-op.
pub fn run() {}

/// Restart the previously running VMs recorded in a node snapshot.
///
/// Each entry is handed to its runtime's plugin: restorable runtimes
/// (externally supervised VMs) relaunch the instance, in-process runtimes
/// acknowledge and skip — the plugin decides, the node does not.
pub fn restore_previously_running_vms(snapshot: &JsonValue) -> Result<JsonValue, String> {
    let runtimes = snapshot["vms"].as_array().cloned().unwrap_or_default();
    let mut restored = 0;
    for vm in runtimes {
        let runtime_hint = vm["runtime"].as_str().unwrap_or("");
        let plugin = vm_registry::get(runtime_hint).or_else(vm_registry::default_plugin);
        if let Some(plugin) = plugin {
            let _ = plugin.restore(&vm);
        }
        restored += 1;
    }
    Ok(json!({"ok": true, "restored": restored}))
}
