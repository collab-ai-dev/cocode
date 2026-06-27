use super::*;

#[test]
fn test_generate_id() {
    let id1 = generate_id("test");
    let id2 = generate_id("test");
    assert!(id1.starts_with("test_"));
    assert_ne!(id1, id2);
}

#[test]
fn test_generate_tool_call_id() {
    let id = generate_tool_call_id();
    assert!(id.starts_with("call_"));
}

#[test]
fn test_generate_random_id() {
    let id = generate_random_id(10);
    assert_eq!(id.len(), 10);
    assert!(id.chars().all(char::is_alphanumeric));
}
