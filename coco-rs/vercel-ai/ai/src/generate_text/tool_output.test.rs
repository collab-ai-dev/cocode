use super::*;
use serde_json::json;

#[test]
fn test_tool_output_text() {
    let output = ToolOutput::text("Hello");
    assert!(output.is_text());
    assert_eq!(output.as_text(), Some("Hello"));
    assert!(!output.is_json());
}

#[test]
fn test_tool_output_json() {
    let output = ToolOutput::json(json!({ "key": "value" }));
    assert!(output.is_json());
    assert!(!output.is_text());
    assert_eq!(output.as_json(), Some(&json!({ "key": "value" })));
}

#[test]
fn test_tool_output_multi() {
    let output = ToolOutput::multi(vec![
        ToolOutputContent::text("Part 1"),
        ToolOutputContent::text("Part 2"),
    ]);
    assert!(!output.is_text());
    assert!(!output.is_json());
}

#[test]
fn test_tool_output_to_json() {
    let text_output = ToolOutput::text("Hello");
    assert_eq!(text_output.to_json(), json!("Hello"));

    let json_output = ToolOutput::json(json!({ "a": 1 }));
    assert_eq!(json_output.to_json(), json!({ "a": 1 }));
}

#[test]
fn test_tool_output_to_string() {
    let text_output = ToolOutput::text("Hello");
    assert_eq!(text_output.to_string_output(), "Hello");

    let json_output = ToolOutput::json(json!({ "a": 1 }));
    assert_eq!(json_output.to_string_output(), r#"{"a":1}"#);
}

#[test]
fn test_tool_output_from() {
    let output1: ToolOutput = "text".into();
    assert!(output1.is_text());

    let output2: ToolOutput = String::from("text").into();
    assert!(output2.is_text());

    let output3: ToolOutput = json!({ "key": "value" }).into();
    assert!(output3.is_json());
}

#[test]
fn test_tool_output_content_text() {
    let content = ToolOutputContent::text("Hello");
    assert!(content.is_text());
    assert!(!content.is_image());
    assert_eq!(content.to_string_output(), "Hello");
}

#[test]
fn test_tool_output_content_image() {
    let content = ToolOutputContent::image("base64data", Some("image/png".to_string()));
    assert!(content.is_image());
    assert!(!content.is_text());
}

#[test]
fn test_tool_output_content_json() {
    let content = ToolOutputContent::json(json!({ "a": 1 }));
    assert!(!content.is_text());
    assert!(!content.is_image());
}
