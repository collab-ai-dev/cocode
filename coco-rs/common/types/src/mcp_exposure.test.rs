use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_default_is_defer() {
    assert_eq!(McpToolExposure::default(), McpToolExposure::Defer);
}

#[test]
fn test_serde_snake_case_roundtrip() {
    for (variant, wire) in [
        (McpToolExposure::Load, "\"load\""),
        (McpToolExposure::Defer, "\"defer\""),
        (McpToolExposure::UseTool, "\"use_tool\""),
    ] {
        assert_eq!(serde_json::to_string(&variant).unwrap(), wire);
        assert_eq!(
            serde_json::from_str::<McpToolExposure>(wire).unwrap(),
            variant
        );
    }
}

#[test]
fn test_restrict_never_widens() {
    use McpToolExposure::{Defer, Load, UseTool};
    // Child may narrow.
    assert_eq!(McpToolExposure::restrict(Load, Defer), Defer);
    assert_eq!(McpToolExposure::restrict(Load, UseTool), UseTool);
    assert_eq!(McpToolExposure::restrict(Defer, UseTool), UseTool);
    // Child may not widen — parent caps it.
    assert_eq!(McpToolExposure::restrict(Defer, Load), Defer);
    assert_eq!(McpToolExposure::restrict(UseTool, Load), UseTool);
    assert_eq!(McpToolExposure::restrict(UseTool, Defer), UseTool);
    // Equal stays.
    assert_eq!(McpToolExposure::restrict(Load, Load), Load);
    assert_eq!(McpToolExposure::restrict(Defer, Defer), Defer);
    assert_eq!(McpToolExposure::restrict(UseTool, UseTool), UseTool);
}

#[test]
fn test_server_override_restriction_uses_each_side_default() {
    use std::collections::HashMap;

    let parent = HashMap::from([
        ("github".into(), McpToolExposure::UseTool),
        ("memory".into(), McpToolExposure::Load),
    ]);
    let requested = HashMap::from([
        ("github".into(), McpToolExposure::Load),
        ("slack".into(), McpToolExposure::UseTool),
    ]);
    let effective = McpToolExposure::restrict_server_overrides(
        McpToolExposure::Defer,
        &parent,
        McpToolExposure::Load,
        &requested,
    );

    assert_eq!(effective.get("github"), Some(&McpToolExposure::UseTool));
    assert_eq!(effective.get("memory"), Some(&McpToolExposure::Load));
    assert_eq!(effective.get("slack"), Some(&McpToolExposure::UseTool));
}
