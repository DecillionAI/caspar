use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::host::functions::protocol_api::forward_host_api_packet;

pub(crate) fn host_fn_create_program(input: &JsonValue) -> String {
    forward_host_api_packet("createProgram", input)
}
