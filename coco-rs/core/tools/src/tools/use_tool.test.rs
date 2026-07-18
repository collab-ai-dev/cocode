use super::*;
use coco_tool_runtime::ToolUseContext;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;

#[test]
fn test_identity_is_use_tool() {
    assert_eq!(UseToolTool.name(), "use_tool");
    assert_eq!(UseToolTool.id(), ToolId::Builtin(ToolName::UseTool));
}

#[test]
fn test_input_parses_name_and_arguments() {
    let input: UseToolInput = serde_json::from_value(json!({
        "name": "mcp__github__create_issue",
        "arguments": { "title": "bug" }
    }))
    .unwrap();
    assert_eq!(input.name, "mcp__github__create_issue");
    assert_eq!(input.arguments, json!({ "title": "bug" }));
}

#[test]
fn test_disabled_in_load_mode() {
    let ctx = ToolUseContext::test_default().with_mcp_tool_exposure(
        coco_types::McpToolExposure::Load,
        Arc::new(Default::default()),
    );
    assert!(!UseToolTool.is_enabled(&ctx));
}

#[test]
fn test_enabled_in_use_tool_mode_with_mcp() {
    let mut features = coco_types::Features::with_defaults();
    features.enable(coco_types::Feature::Mcp);
    let ctx = ToolUseContext::stub_for_filtering(
        Arc::new(features),
        Arc::new(coco_types::ToolOverrides::none()),
        coco_types::ToolFilter::unrestricted(),
        coco_types::PermissionMode::Default,
    )
    .with_mcp_tool_exposure(
        coco_types::McpToolExposure::UseTool,
        Arc::new(Default::default()),
    );
    assert!(UseToolTool.is_enabled(&ctx));
}

#[tokio::test]
async fn test_execute_fails_closed_when_not_unwrapped() {
    // The preparer must unwrap a `use_tool` call to its target; reaching
    // `execute` means the resolver was bypassed, which must error, not no-op.
    let ctx = ToolUseContext::test_default();
    let result = UseToolTool
        .execute(
            UseToolInput {
                name: "mcp__github__create_issue".into(),
                arguments: json!({}),
            },
            &ctx,
        )
        .await;
    assert!(result.is_err(), "carrier execute must fail closed");
}

#[test]
fn test_not_deferred() {
    // The carrier is a static schema, always present in `use_tool` mode.
    assert!(!UseToolTool.should_defer());
}
