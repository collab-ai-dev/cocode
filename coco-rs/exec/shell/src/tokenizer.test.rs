use super::*;
use pretty_assertions::assert_eq;

// ── Basic tokenization ──

#[test]
fn test_tokenize_simple_command() {
    let tokens = tokenize("ls -la /tmp");
    let words: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(words, vec!["ls", "-la", "/tmp"]);
}

#[test]
fn test_tokenize_empty() {
    let tokens = tokenize("");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, TokenKind::Eof);
}

#[test]
fn test_tokenize_whitespace_only() {
    let tokens = tokenize("   \t  ");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].kind, TokenKind::Eof);
}

// ── Operators ──

#[test]
fn test_tokenize_pipe() {
    let tokens = tokenize("cat file | grep pattern");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["|"]);
}

#[test]
fn test_tokenize_and_or() {
    let tokens = tokenize("cmd1 && cmd2 || cmd3");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["&&", "||"]);
}

#[test]
fn test_tokenize_semicolons() {
    let tokens = tokenize("cd /tmp; ls");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec![";"]);
}

#[test]
fn test_tokenize_redirects() {
    let tokens = tokenize("echo hello > file.txt 2>> err.log");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec![">", ">>"]);
}

#[test]
fn test_tokenize_heredoc_operator() {
    let tokens = tokenize("cat <<EOF");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["<<"]);
}

#[test]
fn test_tokenize_herestring_operator() {
    let tokens = tokenize("cat <<< 'hello'");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["<<<"]);
}

#[test]
fn test_tokenize_heredoc_strip_tabs() {
    let tokens = tokenize("cat <<-EOF");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["<<-"]);
}

#[test]
fn test_tokenize_both_redirect() {
    let tokens = tokenize("cmd &> /dev/null");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["&>"]);
}

#[test]
fn test_tokenize_append_both_redirect() {
    let tokens = tokenize("cmd &>> log");
    let ops: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ops, vec!["&>>"]);
}

#[test]
fn test_tokenize_fd_redirect() {
    let tokens = tokenize("cmd 2>&1");
    // "2" is a word, ">&" is an operator
    let has_redir = tokens
        .iter()
        .any(|t| t.kind == TokenKind::Operator && t.value == ">&");
    assert!(has_redir);
}

// ── Quoting ──

#[test]
fn test_tokenize_single_quoted() {
    let tokens = tokenize("echo 'hello world'");
    let squotes: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::SingleQuoted)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(squotes, vec!["'hello world'"]);
}

#[test]
fn test_tokenize_double_quoted() {
    let tokens = tokenize(r#"echo "hello $USER""#);
    let dquotes: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::DoubleQuoted)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(dquotes, vec!["\"hello $USER\""]);
}

#[test]
fn test_tokenize_ansi_c_string() {
    let tokens = tokenize("echo $'hello\\nworld'");
    let ansi: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::AnsiC)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ansi, vec!["$'hello\\nworld'"]);
}

#[test]
fn test_tokenize_escaped_chars_in_word() {
    let tokens = tokenize(r"echo hello\ world");
    // The backslash-space joins the word
    let words: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(words, vec!["echo", "hello\\ world"]);
}

// ── Dollar prefixed ──

#[test]
fn test_tokenize_dollar_paren() {
    let tokens = tokenize("echo $(date)");
    assert!(tokens.iter().any(|t| t.kind == TokenKind::DollarParen));
}

#[test]
fn test_tokenize_dollar_brace() {
    let tokens = tokenize("echo ${HOME}");
    assert!(tokens.iter().any(|t| t.kind == TokenKind::DollarBrace));
}

#[test]
fn test_tokenize_dollar_double_paren() {
    let tokens = tokenize("echo $((1 + 2))");
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::DollarDoubleParen)
    );
}

#[test]
fn test_tokenize_bare_dollar() {
    let tokens = tokenize("echo $USER");
    assert!(tokens.iter().any(|t| t.kind == TokenKind::Dollar));
    // The USER follows as a word
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Word && t.value == "USER")
    );
}

// ── Process substitution ──

#[test]
fn test_tokenize_process_sub_in() {
    let tokens = tokenize("diff <(cat a) <(cat b)");
    let proc_sub_count = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::ProcessSubIn)
        .count();
    assert_eq!(proc_sub_count, 2);
}

// ── Numbers ──

#[test]
fn test_tokenize_numbers() {
    let tokens = tokenize("head -20 file");
    let nums: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Number)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(nums, vec!["-20"]);
}

// ── Comments and newlines ──

#[test]
fn test_tokenize_comment() {
    let tokens = tokenize("echo hello # this is a comment");
    let comments: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Comment)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(comments, vec!["# this is a comment"]);
}

#[test]
fn test_tokenize_newlines() {
    let tokens = tokenize("echo hello\necho world");
    let newlines = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Newline)
        .count();
    assert_eq!(newlines, 1);
}

// ── Line continuation ──

#[test]
fn test_tokenize_line_continuation() {
    let tokens = tokenize("echo \\\nhello");
    // The line continuation should be skipped, leaving "echo hello"
    let words: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(words, vec!["echo", "hello"]);
}

// ── Backtick ──

#[test]
fn test_tokenize_backtick() {
    let tokens = tokenize("echo `date`");
    let backticks = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Backtick)
        .count();
    assert_eq!(backticks, 2);
}

// ── Context-sensitive tokenization ──

#[test]
fn test_tokenize_with_context_bracket() {
    let tokens = tokenize_with_context("[[ -f file ]]");
    let has_double_bracket = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Operator)
        .any(|t| t.value == "[[");
    assert!(has_double_bracket);
}

// ── Expansion detection ──

#[test]
fn test_detect_simple_var() {
    let expansions = detect_expansions("echo $HOME");
    assert_eq!(expansions, vec![Expansion::SimpleVar("HOME".to_string())]);
}

#[test]
fn test_detect_braced_var() {
    let expansions = detect_expansions("echo ${HOME}");
    assert_eq!(expansions, vec![Expansion::BracedVar("HOME".to_string())]);
}

#[test]
fn test_detect_braced_var_with_default() {
    let expansions = detect_expansions("echo ${HOME:-/root}");
    assert_eq!(expansions, vec![Expansion::BracedVar("HOME".to_string())]);
}

#[test]
fn test_detect_command_sub() {
    let expansions = detect_expansions("echo $(date +%Y)");
    assert_eq!(
        expansions,
        vec![Expansion::CommandSub("date +%Y".to_string())]
    );
}

#[test]
fn test_detect_backtick_sub() {
    let expansions = detect_expansions("echo `date`");
    assert_eq!(expansions, vec![Expansion::BacktickSub("date".to_string())]);
}

#[test]
fn test_detect_arithmetic() {
    let expansions = detect_expansions("echo $((1 + 2))");
    assert_eq!(
        expansions,
        vec![Expansion::ArithmeticExp("1 + 2".to_string())]
    );
}

#[test]
fn test_detect_multiple_expansions() {
    let expansions = detect_expansions("echo $HOME/${USER}");
    assert_eq!(
        expansions,
        vec![
            Expansion::SimpleVar("HOME".to_string()),
            Expansion::BracedVar("USER".to_string()),
        ]
    );
}

#[test]
fn test_no_expansion_in_single_quotes() {
    let expansions = detect_expansions("echo '$HOME'");
    assert!(expansions.is_empty());
}

#[test]
fn test_detect_expansion_with_escaped_dollar() {
    let expansions = detect_expansions("echo \\$HOME");
    assert!(expansions.is_empty());
}

// ── has_expansions ──

#[test]
fn test_has_expansions_true() {
    assert!(has_expansions("echo $HOME"));
    assert!(has_expansions("echo $(date)"));
    assert!(has_expansions("echo `date`"));
}

#[test]
fn test_has_expansions_false() {
    assert!(!has_expansions("echo hello"));
    assert!(!has_expansions("echo '$HOME'"));
    assert!(!has_expansions("echo \\$HOME"));
}

// ── has_here_string ──

#[test]
fn test_has_here_string_true() {
    assert!(has_here_string("cat <<< 'hello'"));
    assert!(has_here_string("bc <<< '1+2'"));
}

#[test]
fn test_has_here_string_false() {
    assert!(!has_here_string("cat <<EOF"));
    assert!(!has_here_string("echo hello"));
}

#[test]
fn test_has_here_string_not_in_quotes() {
    assert!(!has_here_string("echo '<<<'"));
    assert!(!has_here_string(r#"echo "<<<""#));
}

// ── Complex cases ──

#[test]
fn test_tokenize_complex_pipeline() {
    let tokens = tokenize("find . -name '*.rs' | xargs grep -l 'TODO' | sort | head -5");
    let words: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word || t.kind == TokenKind::Number)
        .map(|t| t.value.as_str())
        .collect();
    assert!(words.contains(&"find"));
    assert!(words.contains(&"xargs"));
    assert!(words.contains(&"grep"));
    assert!(words.contains(&"sort"));
    assert!(words.contains(&"head"));
    assert!(words.contains(&"-5"));
}

#[test]
fn test_tokenize_assignment() {
    let tokens = tokenize("FOO=bar cargo test");
    let words: Vec<&str> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(words, vec!["FOO=bar", "cargo", "test"]);
}

#[test]
fn test_tokenize_fd_before_redirect() {
    let tokens = tokenize("cmd 2>/dev/null");
    // "2" should be a word (fd number), then ">" operator
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Word && t.value == "2")
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == ">")
    );
}

#[test]
fn test_byte_offsets() {
    let tokens = tokenize("echo hello");
    // "echo" starts at 0, ends at 4
    let echo_tok = &tokens[0];
    assert_eq!(echo_tok.value, "echo");
    assert_eq!(echo_tok.start, 0);
    assert_eq!(echo_tok.end, 4);
    // "hello" starts at 5, ends at 10
    let hello_tok = &tokens[1];
    assert_eq!(hello_tok.value, "hello");
    assert_eq!(hello_tok.start, 5);
    assert_eq!(hello_tok.end, 10);
}
