use super::*;
use crate::parser::ShellParser;
use pretty_assertions::assert_eq;

#[test]
fn test_single_command() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("ls -la");
    let segments = extract_segments_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].command, vec!["ls", "-la"]);
    assert!(!segments[0].is_piped);
}

#[test]
fn test_pipeline() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("cat file | grep pattern | wc -l");
    let segments = extract_segments_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].command, vec!["cat", "file"]);
    assert!(segments[0].is_piped);
    assert_eq!(segments[1].command, vec!["grep", "pattern"]);
    assert!(segments[1].is_piped);
    assert_eq!(segments[2].command, vec!["wc", "-l"]);
    assert!(segments[2].is_piped);
}

#[test]
fn test_and_chain() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("ls && pwd && echo done");
    let segments = extract_segments_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(segments.len(), 3);
    assert!(!segments[0].is_piped);
    assert!(!segments[1].is_piped);
    assert!(!segments[2].is_piped);
}

#[test]
fn test_mixed_pipeline_and_chain() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("cat file | grep pattern && echo done");
    let segments = extract_segments_from_tree(cmd.tree().unwrap(), cmd.source());
    assert_eq!(segments.len(), 3);
    // First two are piped together
    assert!(segments[0].is_piped);
    assert!(segments[1].is_piped);
    // Last one is not piped
    assert!(!segments[2].is_piped);
}

#[test]
fn test_token_fallback() {
    use crate::tokenizer::Tokenizer;
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("cat file | grep pattern").unwrap();
    let segments = extract_segments_from_tokens(&tokens);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].command, vec!["cat", "file"]);
    assert_eq!(segments[1].command, vec!["grep", "pattern"]);
}
