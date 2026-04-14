#[derive(Clone, Debug, Default)]
struct BaseVm {
    machine_id: String,
    vm_id: String,
    runtime: String,
    status: String,
    requester_user_id: String,
    store_id: String,
    created_at_unix_ms: i64,
    updated_at_unix_ms: i64,
}

impl BaseVm {
    fn from_packet(packet: &JsonValue, runtime: &str) -> Self {
        let now = 0_i64;
        BaseVm {
            machine_id: packet["machineId"].as_str().unwrap_or("").to_string(),
            vm_id: packet["vmId"].as_str().unwrap_or("main").to_string(),
            runtime: runtime.to_string(),
            status: "created".to_string(),
            requester_user_id: packet["requesterUserId"].as_str().unwrap_or("").to_string(),
            store_id: packet["storeId"].as_str().unwrap_or("").to_string(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        }
    }
}
