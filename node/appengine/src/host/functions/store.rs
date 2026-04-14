fn host_fn_create_store(input: &JsonValue) -> String {
    forward_host_api_packet("createStore", input)
}

fn host_fn_delete_store(input: &JsonValue) -> String {
    forward_host_api_packet("deleteStore", input)
}
