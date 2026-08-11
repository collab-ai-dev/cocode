use super::*;

#[test]
fn default_requires_approval() {
    assert_eq!(McpExecutionPolicy::default(), McpExecutionPolicy::AlwaysAsk);
}

#[test]
fn serde_uses_explicit_snake_case_values() {
    assert_eq!(
        serde_json::from_str::<McpExecutionPolicy>(r#""trust_read_only_hints""#).unwrap(),
        McpExecutionPolicy::TrustReadOnlyHints
    );
    assert!(serde_json::from_str::<McpExecutionPolicy>(r#""read_only""#).is_err());
}
