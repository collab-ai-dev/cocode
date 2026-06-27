use super::*;

#[test]
fn test_simple_word() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, TokenKind::Word);
    assert_eq!(tokens[0].text, "echo");
}

#[test]
fn test_multiple_words() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo hello world").unwrap();
    let words: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .collect();
    assert_eq!(words.len(), 3);
    assert_eq!(words[0].text, "echo");
    assert_eq!(words[1].text, "hello");
    assert_eq!(words[2].text, "world");
}

#[test]
fn test_single_quoted_string() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo 'hello world'").unwrap();
    let quoted: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::SingleQuoted)
        .collect();
    assert_eq!(quoted.len(), 1);
    assert_eq!(quoted[0].text, "'hello world'");
    assert_eq!(quoted[0].unquoted_content(), "hello world");
}

#[test]
fn test_double_quoted_string() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo \"hello world\"").unwrap();
    let quoted: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::DoubleQuoted)
        .collect();
    assert_eq!(quoted.len(), 1);
    assert_eq!(quoted[0].text, "\"hello world\"");
    assert_eq!(quoted[0].unquoted_content(), "hello world");
}

#[test]
fn test_operators() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("ls && pwd || echo hi").unwrap();
    let ops: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .collect();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0].text, "&&");
    assert_eq!(ops[1].text, "||");
}

#[test]
fn test_pipe() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("cat file | grep pattern").unwrap();
    let ops: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .collect();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].text, "|");
}

#[test]
fn test_redirections() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo hi > file.txt 2>&1").unwrap();
    let redirects: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Redirect)
        .collect();
    assert_eq!(redirects.len(), 2);
    assert_eq!(redirects[0].text, ">");
    assert_eq!(redirects[1].text, "2>&");
}

#[test]
fn test_variable_expansion() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo $HOME ${PATH}").unwrap();
    let vars: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::VariableExpansion)
        .collect();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0].text, "$HOME");
    assert_eq!(vars[1].text, "${PATH}");
}

#[test]
fn test_command_substitution() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo $(pwd) `date`").unwrap();
    let subs: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::CommandSubstitution)
        .collect();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].text, "$(pwd)");
    assert_eq!(subs[1].text, "`date`");
}

#[test]
fn test_ansi_c_quoting() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo $'hello\\nworld'").unwrap();
    let ansi: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::AnsiCQuoted)
        .collect();
    assert_eq!(ansi.len(), 1);
    assert_eq!(ansi[0].text, "$'hello\\nworld'");
}

#[test]
fn test_heredoc() {
    let tokenizer = Tokenizer::new();
    let input = "cat <<'EOF'\nhello\nworld\nEOF\n";
    let tokens = tokenizer.tokenize(input).unwrap();
    let heredocs: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Heredoc)
        .collect();
    assert_eq!(heredocs.len(), 1);
    assert!(heredocs[0].text.contains("hello"));
    assert!(heredocs[0].text.contains("world"));
}

#[test]
fn test_comment() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo hi # this is a comment").unwrap();
    let comments: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Comment)
        .collect();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].text, "# this is a comment");
}
