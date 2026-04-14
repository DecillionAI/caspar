fn host_fn_create_program(input: &JsonValue) -> String {
    forward_host_api_packet("createProgram", input)
}
