use super::*;

fn scoped(name: &str) -> ScopedMcpServerConfig {
    ScopedMcpServerConfig {
        name: name.to_string(),
        config: McpServerConfig::Http(coco_mcp::types::McpHttpConfig {
            url: format!("https://{name}.example.test/mcp"),
            headers: HashMap::new(),
            headers_helper: None,
            oauth: None,
        }),
        scope: coco_mcp::types::ConfigScope::User,
        plugin_source: None,
    }
}

#[test]
fn suggest_server_not_found_uses_closest_match_within_two_edits() {
    let configs = vec![scoped("github"), scoped("linear")];
    assert_eq!(
        suggest_server_not_found("githbu", &configs),
        "No MCP server named \"githbu\". Did you mean \"github\"? Run `coco mcp list` to see all."
    );
}

#[test]
fn suggest_server_not_found_truncates_configured_names() {
    let configs = (0..10)
        .map(|idx| scoped(&format!("server-{idx}")))
        .collect::<Vec<_>>();
    let message = suggest_server_not_found("other", &configs);

    assert!(message.contains("server-0"));
    assert!(message.contains("server-7"));
    assert!(message.contains("(and 2 more; run `coco mcp list` to see all)"));
    assert!(!message.contains("server-9"));
}

#[test]
fn parse_headers_helper_output_requires_string_map() {
    let headers = parse_headers_helper_output("srv", r#"{"Authorization":"Bearer token"}"#)
        .expect("valid string map");
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer token")
    );

    let err = parse_headers_helper_output("srv", r#"{"Authorization":123}"#)
        .expect_err("non-string values are rejected");
    assert!(
        err.to_string()
            .contains("headersHelper for 'srv' returned non-string value")
    );
}

#[test]
fn static_authorization_header_is_case_insensitive() {
    let headers = HashMap::from([("authorization".to_string(), "Bearer token".to_string())]);
    assert!(has_static_authorization(&headers));
}
