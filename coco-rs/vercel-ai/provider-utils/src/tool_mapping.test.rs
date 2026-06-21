use super::*;

#[test]
fn test_tool_mapping_add_get() {
    let mut mapping = ToolMapping::new();
    mapping.add("call_123", "search");

    assert_eq!(mapping.get_name("call_123"), Some("search"));
    assert_eq!(
        mapping.get_ids("search"),
        Some(&["call_123".to_string()][..])
    );
}

#[test]
fn test_tool_mapping_multiple_calls() {
    let mut mapping = ToolMapping::new();
    mapping.add("call_1", "search");
    mapping.add("call_2", "search");
    mapping.add("call_3", "read");

    assert_eq!(
        mapping.get_ids("search"),
        Some(&["call_1".to_string(), "call_2".to_string()][..])
    );
    assert_eq!(mapping.get_ids("read"), Some(&["call_3".to_string()][..]));
    assert_eq!(mapping.len(), 3);
}

#[test]
fn test_tool_mapping_remove() {
    let mut mapping = ToolMapping::new();
    mapping.add("call_1", "search");
    mapping.add("call_2", "search");

    let removed = mapping.remove("call_1");
    assert_eq!(removed, Some("search".to_string()));
    assert_eq!(mapping.get_ids("search"), Some(&["call_2".to_string()][..]));
    assert_eq!(mapping.len(), 1);
}

#[test]
fn test_generate_tool_call_id() {
    let id = generate_tool_call_id("search", 0);
    assert_eq!(id, "search_0");

    let id = generate_tool_call_id("read_file", 42);
    assert_eq!(id, "read_file_42");
}

#[test]
fn test_parse_tool_call_id() {
    let (name, index) = parse_tool_call_id("search_0").unwrap();
    assert_eq!(name, "search");
    assert_eq!(index, 0);

    let (name, index) = parse_tool_call_id("read_file_42").unwrap();
    assert_eq!(name, "read_file");
    assert_eq!(index, 42);
}

#[test]
fn test_parse_tool_call_id_invalid() {
    assert!(parse_tool_call_id("no_underscore_here").is_none());
    assert!(parse_tool_call_id("invalid_notanumber").is_none());
}
