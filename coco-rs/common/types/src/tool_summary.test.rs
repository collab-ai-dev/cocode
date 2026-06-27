use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::ToolName;

#[test]
fn test_bash_summary_is_command() {
    assert_eq!(
        tool_input_summary(ToolName::Bash.as_str(), &json!({"command": "git status"})),
        "git status"
    );
}

#[test]
fn test_grep_summary_is_pattern_in_path() {
    assert_eq!(
        tool_input_summary(
            ToolName::Grep.as_str(),
            &json!({"pattern": "TODO", "path": "src"})
        ),
        "TODO in src"
    );
}

#[test]
fn test_read_summary_with_limit_shows_line_range() {
    assert_eq!(
        tool_input_summary(
            ToolName::Read.as_str(),
            &json!({"file_path": "/repo/README.md", "offset": 5, "limit": 10})
        ),
        "/repo/README.md · lines 5-14"
    );
}

#[test]
fn test_agent_summary_prefers_description() {
    assert_eq!(
        tool_input_summary(
            ToolName::Agent.as_str(),
            &json!({"description": "explore the repo", "prompt": "long prompt body"})
        ),
        "explore the repo"
    );
}

#[test]
fn test_apply_patch_summary_lists_target_paths() {
    let patch = "*** Begin Patch\n*** Update File: a.rs\n*** Add File: b.rs\n*** End Patch";
    assert_eq!(
        tool_input_summary(ToolName::ApplyPatch.as_str(), &json!({"patch": patch})),
        "a.rs, b.rs"
    );
}

#[test]
fn test_unknown_tool_falls_back_to_object_summary() {
    assert_eq!(
        tool_input_summary("totally_custom", &json!({"a": 1, "b": "x"})),
        "a: 1, b: x"
    );
}

#[test]
fn test_mcp_prefixed_name_resolves_on_trailing_segment() {
    // `mcp__server__Read` normalises to the builtin `Read`.
    assert_eq!(
        tool_input_summary("mcp__server__Read", &json!({"file_path": "/x"})),
        "/x"
    );
}

#[test]
fn test_cap_single_line_truncates_with_ellipsis() {
    assert_eq!(cap_single_line("hello world foo", 8), "hello...");
}

#[test]
fn test_partial_primary_arg_extracts_incomplete_bash_command() {
    // Mid-stream: command value not yet closed.
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":"cargo bui"#),
        Some("cargo bui".to_string())
    );
}

#[test]
fn test_partial_primary_arg_extracts_complete_value() {
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":"ls -la"}"#),
        Some("ls -la".to_string())
    );
}

#[test]
fn test_partial_primary_arg_decodes_escapes() {
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":"echo \"hi\""#),
        Some("echo \"hi\"".to_string())
    );
}

#[test]
fn test_partial_primary_arg_none_before_opening_quote() {
    // `"command":` present but the value's opening quote hasn't streamed yet.
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":"#),
        None
    );
}

#[test]
fn test_partial_primary_arg_read_uses_file_path() {
    assert_eq!(
        partial_primary_arg(ToolName::Read.as_str(), r#"{"file_path":"/repo/sr"#),
        Some("/repo/sr".to_string())
    );
}

#[test]
fn test_partial_primary_arg_none_for_tool_without_primary_field() {
    assert_eq!(partial_primary_arg(ToolName::TodoWrite.as_str(), "{"), None);
}

#[test]
fn test_partial_primary_arg_decodes_unicode_escape() {
    // JSON unicode escapes must decode (U+0041 -> 'A'), not render as the
    // literal text "u0041". Built at runtime so the backslash is unambiguous.
    let backslash = '\\';
    let input = format!("{{\"command\":\"{backslash}u0041BC\"}}");
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), &input),
        Some("ABC".to_string())
    );
}

#[test]
fn test_partial_primary_arg_truncated_unicode_escape_waits() {
    // Half a `\u` escape mid-stream: emit what's decoded so far, drop the
    // partial escape (it completes in a later delta).
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":"x\u00"#),
        Some("x".to_string())
    );
}

#[test]
fn test_partial_primary_arg_decodes_backspace_formfeed() {
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":"a\b\fb""#),
        Some("a\u{8}\u{c}b".to_string())
    );
}

#[test]
fn test_partial_primary_arg_empty_value() {
    assert_eq!(
        partial_primary_arg(ToolName::Bash.as_str(), r#"{"command":""#),
        Some(String::new())
    );
}

#[test]
fn test_partial_primary_arg_nested_same_key_is_known_limitation() {
    // Documented best-effort limitation: the scanner matches the FIRST
    // `"command"` occurrence, so a nested object with the same key extracts the
    // inner value. Acceptable — this is a transient cosmetic preview, and real
    // builtin tool schemas are flat. Pinned so a "fix" is a deliberate choice.
    assert_eq!(
        partial_primary_arg(
            ToolName::Bash.as_str(),
            r#"{"x":{"command":"INNER"},"command":"OUTER"}"#
        ),
        Some("INNER".to_string())
    );
}
