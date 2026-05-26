use crate::drivers::vmm::models::vm_runtime::{acquire_resource_lock, release_resource_lock};
use serde_json::{json, Value as JsonValue};

pub(crate) fn host_fn_lock_resource(input: &JsonValue) -> String {
    let resource_id = input["resourceId"].as_str().unwrap_or("");
    let owner_id = input["ownerId"].as_str().unwrap_or("");
    match acquire_resource_lock(resource_id, owner_id) {
        Ok(()) => json!({"ok": true}).to_string(),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

pub(crate) fn host_fn_unlock_resource(input: &JsonValue) -> String {
    let resource_id = input["resourceId"].as_str().unwrap_or("");
    let owner_id = input["ownerId"].as_str().unwrap_or("");
    match release_resource_lock(resource_id, owner_id) {
        Ok(()) => json!({"ok": true}).to_string(),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}
