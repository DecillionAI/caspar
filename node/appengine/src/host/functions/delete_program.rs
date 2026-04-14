fn host_fn_delete_program(input: &JsonValue) -> String {
    forward_host_api_packet("deleteProgram", input)
}
