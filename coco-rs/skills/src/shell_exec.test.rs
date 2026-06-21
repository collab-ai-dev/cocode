use super::*;

#[tokio::test]
async fn test_skip_shell_leaves_prompt_unchanged() {
    // MCP-sourced skills pass skip_shell=true; the body is returned verbatim
    // even when it contains executable patterns.
    let result = execute_shell_in_prompt("Run !`echo hello` here", true).await;
    assert_eq!(result, "Run !`echo hello` here");
}

#[tokio::test]
async fn test_no_patterns_unchanged() {
    let result = execute_shell_in_prompt("No shell here", false).await;
    assert_eq!(result, "No shell here");
}

#[tokio::test]
async fn test_block_pattern_executes_and_substitutes() {
    // ```! block ``` is replaced with the command's stdout.
    let result = execute_shell_in_prompt("before\n```!\necho block_value\n```\nafter", false).await;
    assert_eq!(result, "before\nblock_value\nafter");
}

#[tokio::test]
async fn test_block_pattern_single_line() {
    // Triple-backtick + ! + command on the same fence opener.
    let result = execute_shell_in_prompt("```! echo inline_block ```", false).await;
    assert_eq!(result, "inline_block");
}

#[tokio::test]
async fn test_inline_pattern_executes_at_line_start() {
    let result = execute_shell_in_prompt("!`echo at_start`", false).await;
    assert_eq!(result, "at_start");
}

#[tokio::test]
async fn test_inline_pattern_executes_after_whitespace() {
    let result = execute_shell_in_prompt("value: !`echo spaced`", false).await;
    assert_eq!(result, "value: spaced");
}

#[tokio::test]
async fn test_inline_word_boundary_rejects_attached_bang() {
    // `foo!` has a word char before the `!`, so the lookbehind fails and the
    // span is NOT executed — left verbatim.
    let result = execute_shell_in_prompt("foo!`echo x`", false).await;
    assert_eq!(result, "foo!`echo x`");
}

#[tokio::test]
async fn test_inline_word_boundary_accepts_leading_space() {
    // Same body as the rejected case, but the `!` is preceded by a space.
    let result = execute_shell_in_prompt(" !`echo y`", false).await;
    assert_eq!(result, " y");
}

#[tokio::test]
async fn test_inline_empty_body_left_verbatim() {
    // `[^`]+` requires a non-empty command; an empty span is not a match.
    let result = execute_shell_in_prompt("!``", false).await;
    assert_eq!(result, "!``");
}

#[tokio::test]
async fn test_block_and_inline_combined() {
    let input = "```!\necho B\n```\n then !`echo I`";
    let result = execute_shell_in_prompt(input, false).await;
    assert_eq!(result, "B\n then I");
}

// ── execute_shell_in_prompt_with_tool (handle-routed path) ──

use std::sync::Mutex;

/// Records calls and returns scripted results, mirroring the
/// per-command permission gate.
struct RecordingHandle {
    /// `Ok` output or `Err` message, applied to every call in order.
    behavior: Behavior,
    seen: Mutex<Vec<(String, Vec<String>)>>,
}

enum Behavior {
    /// Echo back the command wrapped, so substitution is observable.
    Echo,
    /// Deny every command with this message.
    Deny(String),
    /// Fail (execution error) every command with this message.
    Fail(String),
}

impl RecordingHandle {
    fn new(behavior: Behavior) -> Self {
        Self {
            behavior,
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl BashToolHandle for RecordingHandle {
    async fn execute_with_permissions(
        &self,
        command: &str,
        allowed_tools: &[String],
    ) -> Result<String, String> {
        self.seen
            .lock()
            .expect("lock")
            .push((command.to_string(), allowed_tools.to_vec()));
        match &self.behavior {
            Behavior::Echo => Ok(format!("<{command}>")),
            Behavior::Deny(m) | Behavior::Fail(m) => Err(m.clone()),
        }
    }
}

#[tokio::test]
async fn with_tool_no_markers_returns_unchanged() {
    let h = RecordingHandle::new(Behavior::Echo);
    let out = execute_shell_in_prompt_with_tool("plain text only", &h, &[])
        .await
        .expect("ok");
    assert_eq!(out, "plain text only");
    assert!(h.seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn with_tool_inline_allow_substitutes() {
    let h = RecordingHandle::new(Behavior::Echo);
    let out = execute_shell_in_prompt_with_tool("a !`echo hi` b", &h, &[])
        .await
        .expect("ok");
    assert_eq!(out, "a <echo hi> b");
    let seen = h.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "echo hi");
}

#[tokio::test]
async fn with_tool_block_allow_substitutes() {
    let h = RecordingHandle::new(Behavior::Echo);
    let out = execute_shell_in_prompt_with_tool("pre\n```!\ngit status\n```\npost", &h, &[])
        .await
        .expect("ok");
    assert_eq!(out, "pre\n<git status>\npost");
    let seen = h.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "git status");
}

#[tokio::test]
async fn with_tool_passes_allowed_tools_through() {
    let h = RecordingHandle::new(Behavior::Echo);
    let allowed = vec!["Bash(git status:*)".to_string()];
    let _ = execute_shell_in_prompt_with_tool("!`git status`", &h, &allowed)
        .await
        .expect("ok");
    let seen = h.seen.lock().expect("lock");
    assert_eq!(seen[0].1, allowed);
}

#[tokio::test]
async fn with_tool_deny_aborts_with_error() {
    let h = RecordingHandle::new(Behavior::Deny("permission denied".into()));
    let err = execute_shell_in_prompt_with_tool("before !`rm -rf /` after", &h, &[])
        .await
        .expect_err("should abort");
    assert_eq!(err, "permission denied");
}

#[tokio::test]
async fn with_tool_failure_aborts_not_silent() {
    // A failing command must abort the whole expansion, not silently
    // substitute empty output.
    let h = RecordingHandle::new(Behavior::Fail("exit 7".into()));
    let err = execute_shell_in_prompt_with_tool("x !`false` y", &h, &[])
        .await
        .expect_err("should abort");
    assert_eq!(err, "exit 7");
}

#[tokio::test]
async fn with_tool_deny_first_marker_stops_before_second() {
    // First marker denies → abort before the second runs (no partial
    // substitution).
    let h = RecordingHandle::new(Behavior::Deny("nope".into()));
    let err = execute_shell_in_prompt_with_tool("!`one` and !`two`", &h, &[])
        .await
        .expect_err("should abort");
    assert_eq!(err, "nope");
    // Only the first command was attempted.
    assert_eq!(h.seen.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn with_tool_multiple_markers_all_substituted() {
    // Each `!` is preceded by whitespace (or start), satisfying the
    // `(?<=^|\s)` lookbehind.
    let h = RecordingHandle::new(Behavior::Echo);
    let out = execute_shell_in_prompt_with_tool("!`a` and !`b`", &h, &[])
        .await
        .expect("ok");
    assert_eq!(out, "<a> and <b>");
}

#[tokio::test]
async fn with_tool_bracketed_inline_not_matched() {
    // `[!`a`]` — `!` preceded by `[` (not whitespace) → not a marker,
    // mirroring the lookbehind. Left verbatim.
    let h = RecordingHandle::new(Behavior::Echo);
    let out = execute_shell_in_prompt_with_tool("[!`a`]", &h, &[])
        .await
        .expect("ok");
    assert_eq!(out, "[!`a`]");
    assert!(h.seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn with_tool_inline_requires_whitespace_before_bang() {
    // `foo!`bar`` — `!` NOT preceded by whitespace/start → not a marker.
    let h = RecordingHandle::new(Behavior::Echo);
    let out = execute_shell_in_prompt_with_tool("foo!`bar`", &h, &[])
        .await
        .expect("ok");
    assert_eq!(out, "foo!`bar`");
    assert!(h.seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn with_tool_noop_handle_blanks_markers() {
    // NoOp handle substitutes empty output and never denies.
    let out = execute_shell_in_prompt_with_tool("a !`echo hi` b", &NoOpBashToolHandle, &[])
        .await
        .expect("ok");
    assert_eq!(out, "a  b");
}

#[test]
fn scan_finds_block_and_inline_deduped() {
    // An inline marker inside a block body must not double-match.
    let markers = scan_shell_markers("```!\n!`inner`\n```\nthen !`outer`");
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].command, "!`inner`");
    assert_eq!(markers[1].command, "outer");
}

#[test]
fn scan_skips_empty_block_command() {
    let markers = scan_shell_markers("```!\n\n```");
    assert!(markers.is_empty());
}
