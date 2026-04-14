fn host_fn_http_request(input: &JsonValue) -> String {
    match perform_http_request(input) {
        Ok(res) => res,
        Err(err) => json!({"ok": false, "error": err}).to_string(),
    }
}
