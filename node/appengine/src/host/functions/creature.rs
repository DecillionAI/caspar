fn host_fn_create_creature(input: &JsonValue) -> String {
    forward_host_api_packet("createCreature", input)
}

fn host_fn_delete_creature(input: &JsonValue) -> String {
    forward_host_api_packet("deleteCreature", input)
}
