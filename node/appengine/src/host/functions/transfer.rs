fn host_fn_transfer(input: &JsonValue) -> String {
    forward_host_api_packet("transfer", input)
}
