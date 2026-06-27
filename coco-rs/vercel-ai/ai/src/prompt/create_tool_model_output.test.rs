use super::*;
use serde_json::json;

#[test]
fn test_create_tool_result_content_string() {
    let value = json!("hello");
    let content = create_tool_result_content(&value);

    // Should be text content
    let serialized = serde_json::to_string(&content).unwrap();
    assert!(serialized.contains("hello"));
}

#[test]
fn test_create_tool_result_content_object() {
    let value = json!({ "key": "value" });
    let content = create_tool_result_content(&value);

    // Should be text content with JSON
    let serialized = serde_json::to_string(&content).unwrap();
    assert!(serialized.contains("key"));
}

#[test]
fn test_create_tool_result_part() {
    let part = create_tool_result_part("call_1", "test_tool", json!({ "result": 42 }), false);

    assert_eq!(part.tool_call_id, "call_1");
    assert_eq!(part.tool_name, "test_tool");
}

#[test]
fn test_create_tool_result_part_from_text() {
    let part = create_tool_result_part_from_text("call_1", "test_tool", "Hello, world!");

    assert_eq!(part.tool_call_id, "call_1");
    assert_eq!(part.tool_name, "test_tool");
}

#[test]
fn test_create_tool_result_part_from_error() {
    let part =
        create_tool_result_part_from_error("call_1", "test_tool", "Something went wrong");

    assert_eq!(part.tool_call_id, "call_1");
    assert_eq!(part.tool_name, "test_tool");
}

#[test]
fn test_create_tool_result_part_from_image() {
    let part =
        create_tool_result_part_from_image("call_1", "test_tool", "base64data", "image/png");

    assert_eq!(part.tool_call_id, "call_1");
    assert_eq!(part.tool_name, "test_tool");
}