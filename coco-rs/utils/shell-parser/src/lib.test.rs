use super::*;

#[test]
fn test_parse_and_analyze() {
    let (cmd, analysis) = parse_and_analyze("ls -la");
    assert!(cmd.has_tree());
    assert!(!analysis.requires_approval());
}

#[test]
fn test_is_safe_command() {
    assert!(is_safe_command("ls -la"));
    assert!(is_safe_command("git status && pwd"));
    assert!(!is_safe_command("rm -rf /"));
    assert!(!is_safe_command("eval $cmd"));
}

#[test]
fn test_full_workflow() {
    let mut parser = ShellParser::new();

    // Parse a pipeline
    let cmd = parser.parse("cat file | grep pattern | wc -l");

    // Extract pipe segments
    if let Some(tree) = cmd.tree() {
        let segments = extract_segments_from_tree(tree, cmd.source());
        assert_eq!(segments.len(), 3);
        assert!(segments[0].is_piped);
    }

    // Check for redirections (none in this case)
    if let Some(tree) = cmd.tree() {
        let redirects = extract_redirects_from_tree(tree, cmd.source());
        assert!(redirects.is_empty());
    }

    // Security analysis
    let analysis = security::analyze(&cmd);
    // Simple pipeline should be relatively safe
    assert!(!analysis.requires_approval());
}

#[test]
fn test_shell_invocation_workflow() {
    let mut parser = ShellParser::new();

    let argv = vec![
        "bash".to_string(),
        "-c".to_string(),
        "echo hello && ls".to_string(),
    ];

    let cmd = parser.parse_shell_invocation(&argv).unwrap();
    let commands = cmd.try_extract_safe_commands().unwrap();

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0], vec!["echo", "hello"]);
    assert_eq!(commands[1], vec!["ls"]);
}

#[test]
fn test_tokenizer_standalone() {
    let tokenizer = Tokenizer::new();
    let tokens = tokenizer.tokenize("echo 'hello' \"world\" $HOME").unwrap();

    // Filter out whitespace for easier testing
    let non_ws: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Whitespace)
        .collect();

    assert_eq!(non_ws.len(), 4);
    assert_eq!(non_ws[0].kind, TokenKind::Word);
    assert_eq!(non_ws[1].kind, TokenKind::SingleQuoted);
    assert_eq!(non_ws[2].kind, TokenKind::DoubleQuoted);
    assert_eq!(non_ws[3].kind, TokenKind::VariableExpansion);
}
