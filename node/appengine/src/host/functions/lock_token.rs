fn host_fn_lock_token(input: &JsonValue) -> String {
    forward_host_api_packet("lockToken", input)
}
