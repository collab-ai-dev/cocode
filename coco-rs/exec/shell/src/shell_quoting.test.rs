use super::*;
use pretty_assertions::assert_eq;

#[test]
fn single_quote_round_trip() {
    assert_eq!(single_quote_for_eval("hello"), "'hello'");
    assert_eq!(single_quote_for_eval(""), "''");
    // The canonical `'"'"'` escape: close-sq, literal-sq-in-dq, reopen-sq.
    assert_eq!(single_quote_for_eval("it's"), r#"'it'"'"'s'"#);
}

#[test]
fn quote_joins_with_space() {
    assert_eq!(quote(&["a", "b c", "d"]), "'a' 'b c' 'd'");
    assert_eq!(quote::<&str>(&[]), "");
}

#[test]
fn quote_posix_only_quotes_when_needed() {
    // Bare path (alphanumerics, `/`, `-`, `_`, `.`) passes through untouched.
    assert_eq!(quote_posix(&["/abs/dir-1_2.x"]), "/abs/dir-1_2.x");
    // Whitespace → single-quote.
    assert_eq!(quote_posix(&["/abs/my dir"]), "'/abs/my dir'");
    // Contains `'` (no `"`/ws) → double-quote, `'` left as-is.
    assert_eq!(quote_posix(&["/abs/it's"]), "\"/abs/it's\"");
    // Unquoted metachars get backslash-escaped.
    assert_eq!(quote_posix(&["a*b?c"]), "a\\*b\\?c");
    // Joined with spaces, each arg quoted independently.
    assert_eq!(quote_posix(&["/x", "b c"]), "/x 'b c'");
    assert_eq!(quote_posix::<&str>(&[]), "");
}

#[test]
fn heredoc_detection() {
    assert!(contains_heredoc("cat <<EOF\nhi\nEOF"));
    assert!(contains_heredoc("cat <<'EOF'\nhi\nEOF"));
    assert!(contains_heredoc("cat <<\"EOF\"\nhi\nEOF"));
    assert!(contains_heredoc("cat <<-EOF\n\thi\nEOF"));
    assert!(contains_heredoc("cat <<-'EOF'\nhi\nEOF"));
    assert!(contains_heredoc("cat <<\\EOF\nhi\nEOF"));

    // Negative cases — bit shifts.
    assert!(!contains_heredoc("echo $((1 << 2))"));
    assert!(!contains_heredoc("[[ 1 << 2 ]]"));
    assert!(!contains_heredoc("x=$((4 << 1))"));

    // Negative case — plain redirect.
    assert!(!contains_heredoc("cat < file.txt"));
}

#[test]
fn multiline_string_detection() {
    assert!(contains_multiline_string("echo 'line1\nline2'"));
    assert!(contains_multiline_string("echo \"line1\nline2\""));

    // Escaped newline in source (literal backslash + n) is NOT multiline.
    assert!(!contains_multiline_string("echo 'no newline here'"));
    assert!(!contains_multiline_string(r#"echo "no newline here""#));

    // Newline outside quotes is NOT multiline-string.
    assert!(!contains_multiline_string("echo hi\necho bye"));
}

#[test]
fn stdin_redirect_detection() {
    assert!(has_stdin_redirect("cat < file"));
    assert!(has_stdin_redirect("cat </dev/null"));
    assert!(has_stdin_redirect("foo | cat < x.txt"));
    assert!(has_stdin_redirect("a; cat < x"));

    // Negative cases.
    assert!(!has_stdin_redirect("cat << EOF"));
    assert!(!has_stdin_redirect("cat <(echo hi)"));
    assert!(!has_stdin_redirect("echo hi"));
}

#[test]
fn should_add_stdin_redirect_logic() {
    assert!(should_add_stdin_redirect("ls -la"));
    // Heredoc skips.
    assert!(!should_add_stdin_redirect("cat <<EOF\nhi\nEOF"));
    // Existing redirect skips.
    assert!(!should_add_stdin_redirect("cat < file.txt"));
}

#[test]
fn quote_shell_command_simple() {
    let out = quote_shell_command("ls -la", /*add_stdin_redirect*/ true);
    assert_eq!(out, "'ls -la' < /dev/null");

    let out_no = quote_shell_command("ls -la", false);
    assert_eq!(out_no, "'ls -la'");
}

#[test]
fn quote_shell_command_heredoc() {
    // Heredoc disables the stdin redirect regardless of the flag.
    let cmd = "cat <<EOF\nhi\nEOF";
    let out = quote_shell_command(cmd, true);
    assert!(
        !out.contains("/dev/null"),
        "heredoc should not get redirect"
    );
    assert!(out.starts_with('\''));
}

#[test]
fn quote_shell_command_escapes_single_quotes() {
    let cmd = "echo 'with quotes'";
    let out = quote_shell_command(cmd, false);
    // The inner single quotes must be escaped.
    assert!(out.contains(r#"'"'"'"#));
}

#[test]
fn rewrite_nul_basic() {
    assert_eq!(rewrite_windows_null_redirect("ls 2>nul"), "ls 2>/dev/null");
    assert_eq!(rewrite_windows_null_redirect("ls > NUL"), "ls > /dev/null");
    assert_eq!(rewrite_windows_null_redirect("ls &>nul"), "ls &>/dev/null");
    assert_eq!(rewrite_windows_null_redirect("ls >>nul"), "ls >>/dev/null");
    // Case insensitive.
    assert_eq!(rewrite_windows_null_redirect("ls 2>Nul"), "ls 2>/dev/null");
}

#[test]
fn rewrite_nul_does_not_match_negatives() {
    assert_eq!(rewrite_windows_null_redirect("cat nul.txt"), "cat nul.txt");
    assert_eq!(rewrite_windows_null_redirect("echo >null"), "echo >null");
    assert_eq!(
        rewrite_windows_null_redirect("echo >nullable"),
        "echo >nullable"
    );
    assert_eq!(
        rewrite_windows_null_redirect("echo >nul.txt"),
        "echo >nul.txt"
    );
}

#[test]
fn rewrite_nul_inside_pipeline() {
    assert_eq!(
        rewrite_windows_null_redirect("foo 2>nul | bar"),
        "foo 2>/dev/null | bar"
    );
    assert_eq!(
        rewrite_windows_null_redirect("foo > nul; bar"),
        "foo > /dev/null; bar"
    );
}
