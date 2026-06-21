use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_extract_valid_tag_parsed() {
    let output = "before\n<claude-code-hint v=\"1\" type=\"plugin\" value=\"foo@anthropic-plugins\" />\nafter";
    let (hints, _) = extract_claude_code_hints(output, "mytool --flag");
    assert_eq!(hints.len(), 1);
    assert_eq!(
        hints[0],
        ClaudeCodeHint {
            v: 1,
            hint_type: "plugin".to_string(),
            value: "foo@anthropic-plugins".to_string(),
            source_command: "mytool".to_string(),
        }
    );
}

#[test]
fn test_extract_tag_line_stripped() {
    let output = "line1\n<claude-code-hint v=\"1\" type=\"plugin\" value=\"foo@bar\" />\nline2";
    let (_, stripped) = extract_claude_code_hints(output, "tool");
    assert!(
        !stripped.contains("<claude-code-hint"),
        "hint tag must be stripped from model-visible output: {stripped:?}"
    );
    assert!(stripped.contains("line1"));
    assert!(stripped.contains("line2"));
}

#[test]
fn test_extract_collapses_3plus_blank_lines() {
    // Two tags on consecutive lines leave blank lines after stripping; the
    // result must collapse runs of 3+ newlines down to 2.
    let output = "a\n\n<claude-code-hint v=\"1\" type=\"plugin\" value=\"x@m\" />\n\n<claude-code-hint v=\"1\" type=\"plugin\" value=\"y@m\" />\n\nb";
    let (hints, stripped) = extract_claude_code_hints(output, "tool");
    assert_eq!(hints.len(), 2);
    assert!(
        !stripped.contains("\n\n\n"),
        "3+ consecutive newlines must collapse to 2: {stripped:?}"
    );
}

#[test]
fn test_extract_unsupported_version_dropped() {
    let output = "<claude-code-hint v=\"2\" type=\"plugin\" value=\"foo@bar\" />";
    let (hints, stripped) = extract_claude_code_hints(output, "tool");
    assert!(hints.is_empty(), "v=2 is unsupported and must be dropped");
    // Even a dropped tag line is stripped from output.
    assert!(!stripped.contains("<claude-code-hint"));
}

#[test]
fn test_extract_unsupported_type_dropped() {
    let output = "<claude-code-hint v=\"1\" type=\"skill\" value=\"foo@bar\" />";
    let (hints, _) = extract_claude_code_hints(output, "tool");
    assert!(hints.is_empty(), "type != plugin must be dropped");
}

#[test]
fn test_extract_empty_value_dropped() {
    let output = "<claude-code-hint v=\"1\" type=\"plugin\" value=\"\" />";
    let (hints, _) = extract_claude_code_hints(output, "tool");
    assert!(hints.is_empty(), "empty value must be dropped");
}

#[test]
fn test_extract_quoted_and_unquoted_attrs() {
    // v is unquoted; type+value quoted. All must parse.
    let output = "<claude-code-hint v=1 type=plugin value=\"foo@bar baz\" />";
    let (hints, _) = extract_claude_code_hints(output, "tool");
    // value has a space -> only quoted form supports it.
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].v, 1);
    assert_eq!(hints[0].hint_type, "plugin");
    assert_eq!(hints[0].value, "foo@bar baz");
}

#[test]
fn test_extract_tag_buried_midline_ignored() {
    // A tag embedded inside a larger line (e.g. a log statement quoting it)
    // is NOT line-anchored, so it must be ignored.
    let output =
        "log: emitted <claude-code-hint v=\"1\" type=\"plugin\" value=\"foo@bar\" /> to stderr";
    let (hints, stripped) = extract_claude_code_hints(output, "tool");
    assert!(hints.is_empty(), "mid-line tag must be ignored");
    assert_eq!(stripped, output, "non-matching output is returned verbatim");
}

#[test]
fn test_extract_leading_trailing_whitespace_tolerated() {
    let output = "   <claude-code-hint v=\"1\" type=\"plugin\" value=\"foo@bar\" />   ";
    let (hints, _) = extract_claude_code_hints(output, "tool");
    assert_eq!(hints.len(), 1, "padded tag lines must still match");
}

#[test]
fn test_extract_fast_path_no_tag() {
    let output = "no tags here\njust text";
    let (hints, stripped) = extract_claude_code_hints(output, "tool");
    assert!(hints.is_empty());
    assert_eq!(stripped, output);
}

#[test]
fn test_first_command_token() {
    assert_eq!(first_command_token("  git status --short  "), "git");
    assert_eq!(first_command_token("single"), "single");
    assert_eq!(first_command_token(""), "");
}

#[test]
fn test_pending_hint_store_set_clear_snapshot() {
    reset_store_for_testing();
    let hint = ClaudeCodeHint {
        v: 1,
        hint_type: "plugin".to_string(),
        value: "foo@bar".to_string(),
        source_command: "tool".to_string(),
    };
    set_pending_hint(hint.clone());
    assert_eq!(pending_hint_snapshot(), Some(hint));
    clear_pending_hint();
    assert_eq!(pending_hint_snapshot(), None);
    reset_store_for_testing();
}

#[test]
fn test_pending_hint_noop_after_shown() {
    reset_store_for_testing();
    mark_shown_this_session();
    assert!(has_shown_hint_this_session());
    let hint = ClaudeCodeHint {
        v: 1,
        hint_type: "plugin".to_string(),
        value: "foo@bar".to_string(),
        source_command: "tool".to_string(),
    };
    set_pending_hint(hint);
    assert_eq!(
        pending_hint_snapshot(),
        None,
        "set_pending_hint is a no-op once shown_this_session is set"
    );
    reset_store_for_testing();
}
