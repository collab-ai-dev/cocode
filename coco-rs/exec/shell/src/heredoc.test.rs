use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_no_heredoc() {
    let (cmd, heredocs) = extract_heredocs("echo hello");
    assert_eq!(cmd, "echo hello");
    assert!(heredocs.is_empty());
}

#[test]
fn test_simple_heredoc() {
    let input = "cat <<EOF\nhello world\nEOF";
    let (cmd, heredocs) = extract_heredocs(input);
    assert_eq!(cmd, "cat");
    assert_eq!(heredocs.len(), 1);
    assert_eq!(heredocs[0].delimiter, "EOF");
    assert_eq!(heredocs[0].content, "hello world");
    assert!(!heredocs[0].is_quoted);
}

#[test]
fn test_quoted_heredoc_single_quotes() {
    let input = "cat <<'EOF'\nhello $world\nEOF";
    let (cmd, heredocs) = extract_heredocs(input);
    assert_eq!(cmd, "cat");
    assert_eq!(heredocs.len(), 1);
    assert_eq!(heredocs[0].delimiter, "EOF");
    assert_eq!(heredocs[0].content, "hello $world");
    assert!(heredocs[0].is_quoted);
}

#[test]
fn test_quoted_heredoc_double_quotes() {
    let input = "cat <<\"END\"\nhello $world\nEND";
    let (cmd, heredocs) = extract_heredocs(input);
    assert_eq!(cmd, "cat");
    assert_eq!(heredocs.len(), 1);
    assert_eq!(heredocs[0].delimiter, "END");
    assert!(heredocs[0].is_quoted);
}

#[test]
fn test_multiline_content() {
    let input = "cat <<EOF\nline1\nline2\nline3\nEOF";
    let (_, heredocs) = extract_heredocs(input);
    assert_eq!(heredocs.len(), 1);
    assert_eq!(heredocs[0].content, "line1\nline2\nline3");
}

#[test]
fn test_command_before_heredoc() {
    let input = "mysql -u root <<SQL\nSELECT 1;\nSQL";
    let (cmd, heredocs) = extract_heredocs(input);
    assert_eq!(cmd, "mysql -u root");
    assert_eq!(heredocs.len(), 1);
    assert_eq!(heredocs[0].delimiter, "SQL");
    assert_eq!(heredocs[0].content, "SELECT 1;");
}

#[test]
fn test_empty_heredoc() {
    let input = "cat <<EOF\nEOF";
    let (_, heredocs) = extract_heredocs(input);
    assert_eq!(heredocs.len(), 1);
    assert_eq!(heredocs[0].content, "");
}

#[test]
fn test_herestring_not_matched() {
    let input = "cat <<<'hello'";
    let (cmd, heredocs) = extract_heredocs(input);
    assert_eq!(cmd, "cat <<<'hello'");
    assert!(heredocs.is_empty());
}

#[test]
fn test_find_heredoc_operator_basic() {
    assert_eq!(find_heredoc_operator("cat <<EOF"), Some(4));
    assert_eq!(find_heredoc_operator("echo hello"), None);
    assert_eq!(find_heredoc_operator("cat <<<word"), None);
}

#[test]
fn test_parse_delimiter_unquoted() {
    let (delim, quoted) = parse_delimiter("EOF");
    assert_eq!(delim, "EOF");
    assert!(!quoted);
}

#[test]
fn test_parse_delimiter_single_quoted() {
    let (delim, quoted) = parse_delimiter("'EOF'");
    assert_eq!(delim, "EOF");
    assert!(quoted);
}

#[test]
fn test_parse_delimiter_double_quoted() {
    let (delim, quoted) = parse_delimiter("\"EOF\"");
    assert_eq!(delim, "EOF");
    assert!(quoted);
}
