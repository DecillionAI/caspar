fn host_fn_consume_lock(input: &JsonValue) -> String {
    forward_host_api_packet("consumeLock", input)
}
