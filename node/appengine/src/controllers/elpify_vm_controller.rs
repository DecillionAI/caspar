struct ElpifyVmController;

impl VmController for ElpifyVmController {
    fn build_image(_packet: &JsonValue) -> Result<JsonValue, String> {
        Ok(json!({"ok": true, "runtime": "elpify", "build": "noop"}))
    }

    fn create(packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        Ok(json!({"ok": true, "runtime": "elpify", "machineId": machine_id}))
    }

    fn starts(packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("").to_string();
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let vm_id = packet["vmId"].as_str().unwrap_or("main").to_string();
        let masm_path = packet["astPath"].as_str().unwrap_or("").to_string();
        let input_raw = packet["input"].as_str().unwrap_or("{}").to_string();
        let vm_handle = {
            let mut map = GLOBAL_ELPIFY_VMS.lock().unwrap();
            Arc::clone(
                map.entry(machine_id.clone())
                    .or_insert_with(|| Arc::new(ElpifyManagedVm::new(machine_id.clone()))),
            )
        };
        vm_handle.enqueue(ElpifyTask {
            masm_path,
            input_raw,
            vm_id,
        })?;
        Ok(json!({"ok": true, "runtime": "elpify", "machineId": machine_id}))
    }

    fn stop(packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        terminate_managed_vm(machine_id);
        Ok(json!({"ok": true, "runtime": "elpify", "machineId": machine_id}))
    }

    fn resume(packet: &JsonValue) -> Result<JsonValue, String> {
        Self::starts(packet)
    }
    fn pause(packet: &JsonValue) -> Result<JsonValue, String> {
        Self::stop(packet)
    }
    fn exec(packet: &JsonValue) -> Result<JsonValue, String> {
        Self::starts(packet)
    }

    fn copy_to(_packet: &JsonValue) -> Result<JsonValue, String> {
        Err("copy_to is not implemented yet for elpify runtime".to_string())
    }

    fn copy_from(_packet: &JsonValue) -> Result<JsonValue, String> {
        Err("copy_from is not implemented yet for elpify runtime".to_string())
    }
}
