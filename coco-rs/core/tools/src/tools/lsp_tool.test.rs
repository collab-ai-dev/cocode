//! Focused tests for `LspTool` dispatch.
//!
//! End-to-end execution requires a real LSP server, which is out of
//! scope for unit tests. These cover the typed-dispatch + validation
//! + handle-gate slices that are independent of the JSON-RPC wire.

use super::*;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use coco_types::Feature;
use serde_json::json;

#[test]
fn lsp_action_lsp_method_round_trip() {
    assert_eq!(
        LspAction::GoToDefinition.lsp_method(),
        "textDocument/definition"
    );
    assert_eq!(
        LspAction::FindReferences.lsp_method(),
        "textDocument/references"
    );
    assert_eq!(LspAction::Hover.lsp_method(), "textDocument/hover");
    assert_eq!(
        LspAction::DocumentSymbol.lsp_method(),
        "textDocument/documentSymbol"
    );
    assert_eq!(LspAction::WorkspaceSymbol.lsp_method(), "workspace/symbol");
    assert_eq!(
        LspAction::GoToImplementation.lsp_method(),
        "textDocument/implementation"
    );
    // The three call-hierarchy variants all start at prepareCallHierarchy;
    // the second-step method is selected inside `dispatch()`.
    assert_eq!(
        LspAction::PrepareCallHierarchy.lsp_method(),
        "textDocument/prepareCallHierarchy"
    );
    assert_eq!(
        LspAction::IncomingCalls.lsp_method(),
        "textDocument/prepareCallHierarchy"
    );
    assert_eq!(
        LspAction::OutgoingCalls.lsp_method(),
        "textDocument/prepareCallHierarchy"
    );
}

#[test]
fn requires_position_matches_schema() {
    // File / workspace scoped operations don't need a position.
    assert!(!LspAction::DocumentSymbol.requires_position());
    assert!(!LspAction::WorkspaceSymbol.requires_position());
    // All others must carry `{line, character}`.
    assert!(LspAction::GoToDefinition.requires_position());
    assert!(LspAction::FindReferences.requires_position());
    assert!(LspAction::Hover.requires_position());
    assert!(LspAction::GoToImplementation.requires_position());
    assert!(LspAction::PrepareCallHierarchy.requires_position());
    assert!(LspAction::IncomingCalls.requires_position());
    assert!(LspAction::OutgoingCalls.requires_position());
}

#[test]
fn build_params_converts_one_based_to_zero_based() {
    let p = build_params(LspAction::GoToDefinition, "file:///a.rs", 10, 5);
    assert_eq!(p["textDocument"]["uri"], "file:///a.rs");
    assert_eq!(p["position"]["line"], 9);
    assert_eq!(p["position"]["character"], 4);
}

#[test]
fn build_params_find_references_includes_declaration() {
    let p = build_params(LspAction::FindReferences, "file:///a.rs", 1, 1);
    assert_eq!(p["context"]["includeDeclaration"], true);
    assert_eq!(p["position"]["line"], 0);
}

#[test]
fn build_params_document_symbol_omits_position() {
    // `line` / `character` are schema-required but ignored for documentSymbol.
    let p = build_params(LspAction::DocumentSymbol, "file:///a.rs", 1, 1);
    assert_eq!(p["textDocument"]["uri"], "file:///a.rs");
    assert!(p.get("position").is_none());
}

#[test]
fn build_params_workspace_symbol_is_empty_query() {
    let p = build_params(LspAction::WorkspaceSymbol, "file:///a.rs", 1, 1);
    assert_eq!(p["query"], "");
    assert!(p.get("textDocument").is_none());
}

#[test]
fn parse_locations_handles_null() {
    assert!(parse_locations(&Value::Null).is_empty());
}

#[test]
fn parse_locations_handles_single_object() {
    let raw = json!({
        "uri": "file:///a.rs",
        "range": {
            "start": { "line": 1, "character": 0 },
            "end":   { "line": 1, "character": 4 }
        }
    });
    let locs = parse_locations(&raw);
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].uri, "file:///a.rs");
}

#[test]
fn parse_locations_handles_location_link_array() {
    // LSP `definition` can return `LocationLink[]` instead of `Location[]`.
    let raw = json!([{
        "originSelectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 4 } },
        "targetUri": "file:///b.rs",
        "targetRange": {
            "start": { "line": 5, "character": 0 },
            "end":   { "line": 5, "character": 8 }
        },
        "targetSelectionRange": {
            "start": { "line": 5, "character": 0 },
            "end":   { "line": 5, "character": 8 }
        }
    }]);
    let locs = parse_locations(&raw);
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].uri, "file:///b.rs");
    assert_eq!(locs[0].range.start.line, 5);
}

#[tokio::test]
async fn is_enabled_requires_both_feature_and_connected_handle() {
    let mut ctx = ToolUseContext::test_default();
    // Feature off + NoOp handle (the default).
    assert!(!<LspTool as DynTool>::is_enabled(&LspTool, &ctx));

    // Feature on, but handle still reports disconnected — still off.
    // `ctx.features` is shared via Arc; build a fresh one with the gate
    // flipped to avoid mutating shared state.
    let mut features = (*ctx.features).clone();
    features.enable(Feature::Lsp);
    ctx.features = std::sync::Arc::new(features);
    assert!(!<LspTool as DynTool>::is_enabled(&LspTool, &ctx));
}

#[tokio::test]
async fn execute_rejects_missing_position() {
    // `line` / `character` are schema-required for every operation —
    // the tool-facing `inputSchema` makes both mandatory. Omitting
    // them is rejected at input deserialization, before dispatch.
    let ctx = ToolUseContext::test_default();
    let input = json!({
        "operation": "goToDefinition",
        "filePath": "/tmp/__lsp_tool_test_no_pos__.rs"
        // no line / character
    });
    let result = <LspTool as DynTool>::execute(&LspTool, input, &ctx).await;
    let Err(err) = result else {
        panic!("expected rejection, got: {result:?}");
    };
    let msg = match err {
        coco_tool_runtime::ToolError::InvalidInput { message, .. } => message,
        other => panic!("expected InvalidInput, got {other:?}"),
    };
    assert!(
        msg.contains("invalid tool input") && msg.contains("line"),
        "missing-position error message changed: {msg}"
    );
}
