use super::*;
use crate::parser::ShellParser;
use pretty_assertions::assert_eq;

#[test]
fn test_output_redirect() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo hi > output.txt");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 1);
    assert_eq!(redirects[0].kind, RedirectKind::Output);
    assert_eq!(redirects[0].target, "output.txt");
    assert!(redirects[0].is_top_level);
}

#[test]
fn test_append_redirect() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo hi >> output.txt");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 1);
    assert_eq!(redirects[0].kind, RedirectKind::Append);
}

#[test]
fn test_input_redirect() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("cat < input.txt");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 1);
    assert_eq!(redirects[0].kind, RedirectKind::Input);
    assert_eq!(redirects[0].target, "input.txt");
}

#[test]
fn test_fd_redirect() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("command 2>&1");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 1);
    assert_eq!(redirects[0].kind, RedirectKind::Duplicate);
    assert_eq!(redirects[0].fd, Some(2));
}

#[test]
fn test_multiple_redirects() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("command < input.txt > output.txt 2>&1");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 3);
}

#[test]
fn test_heredoc() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("cat <<'EOF'\nhello\nEOF");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 1);
    assert_eq!(redirects[0].kind, RedirectKind::HereDoc);
    assert_eq!(redirects[0].target, "EOF");
}

#[test]
fn test_redirect_in_subshell() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("(echo hi > output.txt)");
    let redirects = extract_redirects_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(redirects.len(), 1);
    assert!(!redirects[0].is_top_level);
}

#[test]
fn test_writes_to_file() {
    let redirect = Redirect::new(
        RedirectKind::Output,
        "file.txt".to_string(),
        None,
        Span::new(0, 10),
        true,
    );
    assert!(redirect.writes_to_file());

    let redirect2 = Redirect::new(
        RedirectKind::Duplicate,
        "&1".to_string(),
        Some(2),
        Span::new(0, 10),
        true,
    );
    assert!(!redirect2.writes_to_file());
}
