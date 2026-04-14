fn host_fn_create_access(input: &JsonValue) -> String {
    forward_host_api_packet("createAccess", input)
}

fn host_fn_delete_access(input: &JsonValue) -> String {
    forward_host_api_packet("deleteAccess", input)
}
