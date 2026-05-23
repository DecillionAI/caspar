use crate::drivers::vmm::appengine::prelude::*;
use crate::drivers::vmm::appengine::host::functions::protocol_api::forward_host_api_packet;

pub(crate) fn host_fn_consume_lock(input: &JsonValue) -> String {
    forward_host_api_packet("consumeLock", input)
}
