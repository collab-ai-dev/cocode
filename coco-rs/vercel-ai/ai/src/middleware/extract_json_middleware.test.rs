use super::*;

#[test]
fn test_default_transform_strips_json_fence() {
    let input = "```json\n{\"key\": \"value\"}\n```";
    let result = default_transform(input);
    assert_eq!(result, "{\"key\": \"value\"}");
}

#[test]
fn test_default_transform_strips_generic_fence() {
    let input = "```\n{\"key\": \"value\"}\n```";
    let result = default_transform(input);
    assert_eq!(result, "{\"key\": \"value\"}");
}

#[test]
fn test_default_transform_handles_plain_json() {
    let input = "{\"key\": \"value\"}";
    let result = default_transform(input);
    assert_eq!(result, "{\"key\": \"value\"}");
}

#[test]
fn test_default_transform_handles_no_newline() {
    let input = "```json{\"key\": \"value\"}```";
    let result = default_transform(input);
    assert_eq!(result, "{\"key\": \"value\"}");
}
