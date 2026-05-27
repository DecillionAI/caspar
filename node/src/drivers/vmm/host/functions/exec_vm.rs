use crate::drivers::vmm::prelude::*;

pub(crate) fn host_fn_exec_vm(input: &JsonValue) -> String {
    let mut packet = input.clone();
    if let JsonValue::Object(map) = &mut packet {
        map.insert("type".to_string(), JsonValue::String("execVm".to_string()));
    }
    crate::drivers::vmm::dispatch_packet(&packet)
}
