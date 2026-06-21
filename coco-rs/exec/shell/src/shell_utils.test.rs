use super::*;
use pretty_assertions::assert_eq;

// ── quote_arg ──

#[test]
fn test_quote_arg_empty() {
    assert_eq!(quote_arg(""), "''");
}

#[test]
fn test_quote_arg_simple() {
    // No special chars — returned unquoted
    assert_eq!(quote_arg("hello"), "hello");
    assert_eq!(quote_arg("/usr/bin/git"), "/usr/bin/git");
    assert_eq!(quote_arg("file.rs"), "file.rs");
}

#[test]
fn test_quote_arg_with_spaces() {
    assert_eq!(quote_arg("hello world"), "'hello world'");
}

#[test]
fn test_quote_arg_with_single_quote() {
    assert_eq!(quote_arg("it's"), "'it'\"'\"'s'");
}

#[test]
fn test_quote_arg_with_special_chars() {
    assert_eq!(quote_arg("a$b"), "'a$b'");
    assert_eq!(quote_arg("a;b"), "'a;b'");
    assert_eq!(quote_arg("a|b"), "'a|b'");
    assert_eq!(quote_arg("a&b"), "'a&b'");
}

// ── escape_for_double_quotes ──

#[test]
fn test_escape_for_double_quotes() {
    assert_eq!(escape_for_double_quotes("hello"), "hello");
    assert_eq!(escape_for_double_quotes("$HOME"), "\\$HOME");
    assert_eq!(escape_for_double_quotes("a\"b"), "a\\\"b");
    assert_eq!(escape_for_double_quotes("a`cmd`b"), "a\\`cmd\\`b");
    assert_eq!(escape_for_double_quotes("a\\b"), "a\\\\b");
    assert_eq!(escape_for_double_quotes("!history"), "\\!history");
}

// ── escape_for_bash ──

#[test]
fn test_escape_for_bash_simple() {
    assert_eq!(escape_for_bash("hello"), "hello");
}

#[test]
fn test_escape_for_bash_metacharacters() {
    assert_eq!(escape_for_bash("a b"), "a\\ b");
    assert_eq!(escape_for_bash("a;b"), "a\\;b");
    assert_eq!(escape_for_bash("a|b"), "a\\|b");
    assert_eq!(escape_for_bash("$HOME"), "\\$HOME");
}

// ── has_stdin_redirect ──

#[test]
fn test_has_stdin_redirect_true() {
    assert!(has_stdin_redirect("cat < file.txt"));
    assert!(has_stdin_redirect("cmd < /dev/null"));
}

#[test]
fn test_has_stdin_redirect_false_heredoc() {
    assert!(!has_stdin_redirect("cat <<EOF"));
}

#[test]
fn test_has_stdin_redirect_false_process_sub() {
    assert!(!has_stdin_redirect("diff <(cat a) <(cat b)"));
}

#[test]
fn test_has_stdin_redirect_false_no_redirect() {
    assert!(!has_stdin_redirect("echo hello"));
}

// ── should_add_stdin_redirect ──

#[test]
fn test_should_add_stdin_redirect() {
    assert!(should_add_stdin_redirect("echo hello"));
    assert!(!should_add_stdin_redirect("cat < input.txt"));
    assert!(!should_add_stdin_redirect("cat <<EOF\nhello\nEOF"));
}

// ── quote_shell_command ──

#[test]
fn test_quote_shell_command_simple() {
    let result = quote_shell_command("echo hello", /*add_stdin_redirect*/ true);
    assert!(result.contains("< /dev/null"));
}

#[test]
fn test_quote_shell_command_no_redirect() {
    let result = quote_shell_command("echo hello", /*add_stdin_redirect*/ false);
    assert!(!result.contains("< /dev/null"));
}

#[test]
fn test_quote_shell_command_heredoc_no_redirect() {
    let result = quote_shell_command("cat <<EOF\nhello\nEOF", /*add_stdin_redirect*/ true);
    // Heredocs should not get stdin redirect
    assert!(!result.contains("< /dev/null"));
}

// ── rewrite_windows_null_redirect ──

#[test]
fn test_rewrite_nul_to_dev_null() {
    assert_eq!(rewrite_windows_null_redirect("cmd >nul"), "cmd >/dev/null");
}

#[test]
fn test_rewrite_nul_case_insensitive() {
    assert_eq!(rewrite_windows_null_redirect("cmd >NUL"), "cmd >/dev/null");
}

#[test]
fn test_rewrite_nul_with_fd() {
    assert_eq!(
        rewrite_windows_null_redirect("cmd 2>nul"),
        "cmd 2>/dev/null"
    );
}

#[test]
fn test_rewrite_nul_does_not_match_null() {
    // "null" (with two l's) should not be rewritten
    let input = "cmd >null";
    assert_eq!(rewrite_windows_null_redirect(input), input);
}

// ── split_command_segments ──

#[test]
fn test_split_single_command() {
    let segments = split_command_segments("ls -la");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].command, "ls -la");
    assert_eq!(segments[0].separator, None);
}

#[test]
fn test_split_and_chain() {
    let segments = split_command_segments("cd /tmp && ls -la");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].command, "cd /tmp");
    assert_eq!(segments[0].separator, Some("&&".to_string()));
    assert_eq!(segments[1].command, "ls -la");
    assert_eq!(segments[1].separator, None);
}

#[test]
fn test_split_pipe_chain() {
    let segments = split_command_segments("cat file | grep pat | wc -l");
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].command, "cat file");
    assert_eq!(segments[0].separator, Some("|".to_string()));
    assert_eq!(segments[1].command, "grep pat");
    assert_eq!(segments[1].separator, Some("|".to_string()));
    assert_eq!(segments[2].command, "wc -l");
}

#[test]
fn test_split_semicolons() {
    let segments = split_command_segments("echo hello; echo world");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].command, "echo hello");
    assert_eq!(segments[0].separator, Some(";".to_string()));
    assert_eq!(segments[1].command, "echo world");
}

#[test]
fn test_split_mixed_operators() {
    let segments = split_command_segments("cmd1 && cmd2 | cmd3 ; cmd4");
    assert_eq!(segments.len(), 4);
    assert_eq!(segments[0].separator, Some("&&".to_string()));
    assert_eq!(segments[1].separator, Some("|".to_string()));
    assert_eq!(segments[2].separator, Some(";".to_string()));
    assert_eq!(segments[3].separator, None);
}

// ── first_command ──

#[test]
fn test_first_command() {
    assert_eq!(first_command("echo hello"), Some("echo hello".to_string()));
    assert_eq!(first_command("cd /tmp && ls"), Some("cd /tmp".to_string()));
    assert_eq!(
        first_command("cat file | grep pat"),
        Some("cat file".to_string())
    );
}

#[test]
fn test_first_command_empty() {
    assert_eq!(first_command(""), None);
    assert_eq!(first_command("   "), None);
}

// ── detect_shell ──

#[test]
fn test_detect_shell_bash() {
    assert_eq!(detect_shell("/bin/bash"), ShellKind::Bash);
    assert_eq!(detect_shell("/usr/local/bin/bash"), ShellKind::Bash);
    assert_eq!(detect_shell("bash"), ShellKind::Bash);
}

#[test]
fn test_detect_shell_zsh() {
    assert_eq!(detect_shell("/bin/zsh"), ShellKind::Zsh);
    assert_eq!(detect_shell("/usr/bin/zsh"), ShellKind::Zsh);
    assert_eq!(detect_shell("zsh"), ShellKind::Zsh);
}

#[test]
fn test_detect_shell_fish() {
    assert_eq!(detect_shell("/usr/bin/fish"), ShellKind::Fish);
    assert_eq!(detect_shell("fish"), ShellKind::Fish);
}

#[test]
fn test_detect_shell_sh() {
    assert_eq!(detect_shell("/bin/sh"), ShellKind::Sh);
}

#[test]
fn test_detect_shell_dash() {
    assert_eq!(detect_shell("/bin/dash"), ShellKind::Dash);
}

#[test]
fn test_detect_shell_unknown() {
    assert_eq!(detect_shell("/bin/tcsh"), ShellKind::Unknown);
    assert_eq!(detect_shell("pwsh"), ShellKind::Unknown);
}

// ── disable_extglob_command ──

#[test]
fn test_disable_extglob_bash() {
    assert!(disable_extglob_command(ShellKind::Bash).is_some());
}

#[test]
fn test_disable_extglob_zsh() {
    assert!(disable_extglob_command(ShellKind::Zsh).is_some());
}

#[test]
fn test_disable_extglob_unknown() {
    assert!(disable_extglob_command(ShellKind::Unknown).is_none());
}

// ── CWD tracking ──

#[test]
fn test_cwd_tracking_command() {
    let cmd = cwd_tracking_command();
    assert!(cmd.contains(CWD_MARKER_PREFIX));
    assert!(cmd.contains(CWD_MARKER_SUFFIX));
    assert!(cmd.starts_with("echo "));
}

#[test]
fn test_extract_cwd_from_output() {
    let line = format!("{CWD_MARKER_PREFIX}/home/user/project{CWD_MARKER_SUFFIX}");
    assert_eq!(extract_cwd_from_output(&line), Some("/home/user/project"));
}

#[test]
fn test_extract_cwd_from_output_with_whitespace() {
    let line = format!("  {CWD_MARKER_PREFIX}/tmp{CWD_MARKER_SUFFIX}  ");
    assert_eq!(extract_cwd_from_output(&line), Some("/tmp"));
}

#[test]
fn test_extract_cwd_from_output_no_markers() {
    assert_eq!(extract_cwd_from_output("normal output"), None);
    assert_eq!(extract_cwd_from_output(""), None);
}

#[test]
fn test_extract_cwd_from_output_partial_markers() {
    let line = format!("{CWD_MARKER_PREFIX}/tmp");
    assert_eq!(extract_cwd_from_output(&line), None);
}
