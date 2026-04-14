fn host_fn_deploy_entity(input: &JsonValue) -> String {
    forward_host_api_packet("deployEntity", input)
}
