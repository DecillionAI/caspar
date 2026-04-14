fn host_fn_validate_sign(input: &JsonValue) -> String {
    forward_host_api_packet("validateSign", input)
}
