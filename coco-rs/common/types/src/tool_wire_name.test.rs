use pretty_assertions::assert_eq;

use super::*;
use crate::ToolName;

fn mcp(server: &str, tool: &str) -> ToolId {
    ToolId::Mcp {
        server: server.to_string(),
        tool: tool.to_string(),
    }
}

#[test]
fn test_builtin_uses_canonical_name() {
    let w = WireToolName::for_tool_id(&ToolId::Builtin(ToolName::Read));
    assert_eq!(w.as_str(), "Read");
}

#[test]
fn test_custom_passes_through() {
    let w = WireToolName::for_tool_id(&ToolId::Custom("my_plugin_tool".to_string()));
    assert_eq!(w.as_str(), "my_plugin_tool");
}

#[test]
fn test_invalid_custom_name_gets_provider_safe_handle() {
    let w = WireToolName::for_tool_id(&ToolId::Custom("custom tool/危险".to_string()));
    assert!(is_wire_valid(w.as_str()));
    assert!(w.as_str().starts_with("tool__"));
    assert!(!w.as_str().starts_with("mcp__"));
    assert_ne!(w.as_str(), "custom tool/危险");
}

#[test]
fn test_mcp_short_name_stays_readable() {
    let w = WireToolName::for_tool_id(&mcp("github", "create_issue"));
    assert_eq!(w.as_str(), "mcp__github__create_issue");
}

#[test]
fn test_same_bare_tool_different_servers_are_distinct() {
    // The core collision the discovery layer must stop conflating.
    let gh = WireToolName::for_tool_id(&mcp("github", "create_issue"));
    let gl = WireToolName::for_tool_id(&mcp("gitlab", "create_issue"));
    assert_ne!(gh, gl);
    assert_eq!(gh.as_str(), "mcp__github__create_issue");
    assert_eq!(gl.as_str(), "mcp__gitlab__create_issue");
}

#[test]
fn test_overlong_name_falls_back_to_bounded_handle() {
    let long_tool = "a".repeat(80);
    let w = WireToolName::for_tool_id(&mcp("srv", &long_tool));
    assert!(w.as_str().len() <= MAX_WIRE_TOOL_NAME_BYTES);
    assert!(w.as_str().starts_with("mcp__"));
    // Readable prefix retained (truncated), then the hash suffix.
    assert!(w.as_str().contains("aaaa"));
    assert!(is_wire_valid(w.as_str()));
}

#[test]
fn test_handle_charset_always_valid() {
    // Non-ASCII in the name forces the hashed handle; the result must still be
    // provider-safe.
    let w = WireToolName::for_tool_id(&mcp("sérvør", "wîdget"));
    assert!(is_wire_valid(w.as_str()));
    assert!(w.as_str().len() <= MAX_WIRE_TOOL_NAME_BYTES);
}

#[test]
fn test_generation_is_deterministic() {
    let id = mcp("srv", &"z".repeat(90));
    let a = WireToolName::for_tool_id(&id);
    let b = WireToolName::for_tool_id(&id);
    assert_eq!(a, b);
}

#[test]
fn test_hashed_handles_distinguish_by_full_identity() {
    // Two servers, same overlong bare tool name → still distinct handles,
    // because the suffix hashes the full qualified id.
    let tool = "x".repeat(90);
    let a = WireToolName::for_tool_id(&mcp("github", &tool));
    let b = WireToolName::for_tool_id(&mcp("gitlab", &tool));
    assert_ne!(a, b);
}

#[test]
fn test_exact_length_boundary_stays_natural() {
    // mcp__ (5) + server + __ (2) + tool == 64 → natural name kept.
    // server="s" (1), so tool budget = 64 - 5 - 1 - 2 = 56.
    let tool = "t".repeat(56);
    let id = mcp("s", &tool);
    let natural = id.to_string();
    assert_eq!(natural.len(), MAX_WIRE_TOOL_NAME_BYTES);
    let w = WireToolName::for_tool_id(&id);
    assert_eq!(w.as_str(), natural);
}

#[test]
fn test_one_over_boundary_falls_back() {
    let tool = "t".repeat(57);
    let id = mcp("s", &tool);
    assert_eq!(id.to_string().len(), MAX_WIRE_TOOL_NAME_BYTES + 1);
    let w = WireToolName::for_tool_id(&id);
    assert!(w.as_str().len() <= MAX_WIRE_TOOL_NAME_BYTES);
    assert_ne!(w.as_str(), id.to_string());
}
