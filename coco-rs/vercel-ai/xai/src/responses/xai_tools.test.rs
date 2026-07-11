use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;

#[test]
fn web_search_wire_omits_absent_fields() {
    let args = XaiWebSearchArgs {
        allowed_domains: Some(vec!["a.com".into()]),
        enable_image_search: Some(true),
        ..Default::default()
    };
    let wire = args.to_wire();
    assert_eq!(wire["type"], "web_search");
    assert_eq!(wire["allowed_domains"][0], "a.com");
    assert_eq!(wire["enable_image_search"], true);
    assert!(wire.get("excluded_domains").is_none());
    assert!(wire.get("enable_image_understanding").is_none());
}

#[test]
fn x_search_wire_maps_dates() {
    let args = XaiXSearchArgs {
        from_date: Some("2024-01-01".into()),
        to_date: Some("2024-02-01".into()),
        enable_video_understanding: Some(true),
        ..Default::default()
    };
    let wire = args.to_wire();
    assert_eq!(wire["type"], "x_search");
    assert_eq!(wire["from_date"], "2024-01-01");
    assert_eq!(wire["to_date"], "2024-02-01");
    assert_eq!(wire["enable_video_understanding"], true);
}

#[test]
fn file_search_wire() {
    let args = XaiFileSearchArgs {
        vector_store_ids: Some(vec!["vs_1".into()]),
        max_num_results: Some(3),
    };
    let wire = args.to_wire();
    assert_eq!(wire["type"], "file_search");
    assert_eq!(wire["vector_store_ids"][0], "vs_1");
    assert_eq!(wire["max_num_results"], 3);
}

#[test]
fn mcp_wire() {
    let args = XaiMcpServerArgs {
        server_url: Some("https://mcp.example".into()),
        server_label: Some("example".into()),
        allowed_tools: Some(vec!["t1".into()]),
        ..Default::default()
    };
    let wire = args.to_wire();
    assert_eq!(wire["type"], "mcp");
    assert_eq!(wire["server_url"], "https://mcp.example");
    assert_eq!(wire["server_label"], "example");
    assert_eq!(wire["allowed_tools"][0], "t1");
    assert!(wire.get("authorization").is_none());
}

#[test]
fn parse_tool_args_from_camel_case_map() {
    let mut map: HashMap<String, serde_json::Value> = HashMap::new();
    map.insert(
        "allowedDomains".into(),
        serde_json::json!(["a.com", "b.com"]),
    );
    let args: XaiWebSearchArgs = parse_tool_args(&map);
    assert_eq!(
        args.allowed_domains,
        Some(vec!["a.com".to_string(), "b.com".to_string()])
    );
}

#[test]
fn parse_tool_args_defaults_on_empty() {
    let map: HashMap<String, serde_json::Value> = HashMap::new();
    let args: XaiFileSearchArgs = parse_tool_args(&map);
    assert_eq!(args.vector_store_ids, None);
    assert_eq!(args.max_num_results, None);
}
