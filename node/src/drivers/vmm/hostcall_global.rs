//! Translation of `drivers/vmm/hostcall_global.go`.

use serde_json::Value;

use super::vmm::{check_i64, check_str, Vmm};

impl Vmm {
    /// Entry point: parse a host-call message coming from the appengine over
    /// ZMQ and route it to the right handler. Returns `(result_json,
    /// requestId)` so the caller can produce an `apiResponse` envelope.
    pub fn vm_callback(&self, data_raw: &str) -> (String, i64) {
        let data: Value = match serde_json::from_str(data_raw) {
            Ok(v) => v,
            Err(e) => return (format!("{{\"error\":\"{}\"}}", e), 0),
        };
        let req_id = check_i64(&data, "requestId", 0);
        let key = check_str(&data, "key", "");
        let input = data.get("input").cloned().unwrap_or(Value::Null);

        match key.as_str() {
            "execDocker" | "copyToDocker" | "execVm" | "copyToVm" | "runVm" => {
                ("unsupported runtime".into(), req_id)
            }
            "checkTokenValidity" => self.handle_check_token_validity(&input, req_id),
            "plantTrigger" => self.handle_plant_trigger(&input, req_id),
            "signal" => self.handle_signal_store(&input, req_id),
            "terminateVm" => self.handle_terminate_vm(&input, req_id),
            "sendMessageOnChain" => self.handle_send_message_on_chain(&input, req_id),
            "createCreature" => self.handle_creature_crud("create", &input, req_id),
            "updateCreature" => self.handle_creature_crud("update", &input, req_id),
            "deleteCreature" => self.handle_creature_crud("delete", &input, req_id),
            "getCreature" => self.handle_creature_crud("get", &input, req_id),
            "listCreatures" => self.handle_creature_crud("list", &input, req_id),
            "createResourceStore" | "createVmOwnedStore" => {
                self.handle_resource_store_crud("create", &input, req_id)
            }
            "updateResourceStore" | "updateVmOwnedStore" => {
                self.handle_resource_store_crud("update", &input, req_id)
            }
            "deleteResourceStore" | "deleteVmOwnedStore" => {
                self.handle_resource_store_crud("delete", &input, req_id)
            }
            "getResourceStore" | "getVmOwnedStore" => {
                self.handle_resource_store_crud("get", &input, req_id)
            }
            "listResourceStores" | "listVmOwnedStores" => {
                self.handle_resource_store_crud("list", &input, req_id)
            }
            "createStore" => self.handle_store_crud("create", &input, req_id),
            "updateStore" => self.handle_store_crud("update", &input, req_id),
            "deleteStore" => self.handle_store_crud("delete", &input, req_id),
            "getStore" => self.handle_store_crud("get", &input, req_id),
            "listStores" => self.handle_store_crud("list", &input, req_id),
            "createResourceEntity" => self.handle_resource_entity_create(&input, req_id),
            "deleteResourceEntity" => self.handle_resource_entity_delete(&input, req_id),
            "createWorkchain" | "deleteWorkchain" | "createSubchain" | "deleteSubchain" => {
                self.handle_vm_chain_request(&key, &input, req_id)
            }
            "execShellAction" => self.handle_exec_shell_action(&input, req_id),
            "genId" | "getLink" | "delKey" | "createAccess" | "deleteAccess" | "getJson"
            | "putJson" | "getByPrefix" | "hasAccessToStore" | "signalUser" | "signalGroup"
            | "joinGroup" => self.handle_micro_host_action(&key, &input, req_id),
            "log" | "vmLog" | "buildLog" | "output" | "vmOutput" => {
                self.handle_vm_log_event(&input, req_id)
            }
            _ => ("{}".into(), req_id),
        }
    }
}
