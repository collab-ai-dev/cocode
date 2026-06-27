use super::*;

#[test]
fn test_process_command() {
    let input = process_user_input("/compact");
    assert!(input.is_command);
    assert_eq!(input.command_name, Some("compact".to_string()));
    assert_eq!(input.command_args, None);
}

#[test]
fn test_process_command_with_args() {
    let input = process_user_input("/model sonnet");
    assert!(input.is_command);
    assert_eq!(input.command_name, Some("model".to_string()));
    assert_eq!(input.command_args, Some("sonnet".to_string()));
}

#[test]
fn test_process_normal_text() {
    let input = process_user_input("Hello, world!");
    assert!(!input.is_command);
    assert_eq!(input.text, "Hello, world!");
}

#[test]
fn test_extract_file_mention() {
    let input = process_user_input("Look at @src/main.rs");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "src/main.rs");
    assert_eq!(input.mentions[0].mention_type, MentionType::FilePath);
    assert_eq!(input.mentions[0].line_start, None);
    assert_eq!(input.mentions[0].line_end, None);
}

#[test]
fn test_extract_url_mention() {
    let input = process_user_input("Check @https://example.com");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].mention_type, MentionType::Url);
}

#[test]
fn test_empty_input() {
    let input = process_user_input("");
    assert!(!input.is_command);
    assert!(input.text.is_empty());
}

// --- Quoted path tests ---

#[test]
fn test_quoted_path_mention() {
    let input = process_user_input("Look at @\"path with spaces/file.rs\"");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "path with spaces/file.rs");
    assert_eq!(input.mentions[0].mention_type, MentionType::FilePath);
}

#[test]
fn test_quoted_path_with_line_range() {
    let input = process_user_input("Check @\"src/lib.rs\"#L10-20");
    // The quoted path parser consumes until closing ", so the #L is outside
    // For quoted paths the line range would need to be inside the quotes
    // @"file"#L10-20 — the # is after closing quote
    // Our parser: @"file" stops at ", so #L10-20 is not part of the mention
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "src/lib.rs");
}

// --- Line range tests ---

#[test]
fn test_line_range_single_line() {
    let input = process_user_input("Look at @src/main.rs#L10");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "src/main.rs");
    assert_eq!(input.mentions[0].mention_type, MentionType::FilePath);
    assert_eq!(input.mentions[0].line_start, Some(10));
    // `#L10` alone defaults `lineEnd` to `lineStart`.
    assert_eq!(input.mentions[0].line_end, Some(10));
}

#[test]
fn test_line_range_span() {
    let input = process_user_input("Look at @src/main.rs#L10-20");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "src/main.rs");
    assert_eq!(input.mentions[0].line_start, Some(10));
    assert_eq!(input.mentions[0].line_end, Some(20));
}

#[test]
fn test_line_range_invalid_fragment_ignored() {
    let input = process_user_input("Look at @src/main.rs#heading");
    assert_eq!(input.mentions.len(), 1);
    // Non-#L fragments are stripped from the path.
    assert_eq!(input.mentions[0].text, "src/main.rs");
    assert_eq!(input.mentions[0].line_start, None);
    assert_eq!(input.mentions[0].line_end, None);
}

// --- Agent mention tests ---

#[test]
fn test_agent_mention() {
    let input = process_user_input("Ask @agent-code-reviewer to check");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "agent-code-reviewer");
    assert_eq!(input.mentions[0].mention_type, MentionType::Agent);
}

#[test]
fn test_quoted_agent_mention() {
    let input = process_user_input("Ask @\"code-reviewer (agent)\" to check");
    assert_eq!(input.mentions.len(), 1);
    // ` (agent)` suffix is stripped from the mention text.
    assert_eq!(input.mentions[0].text, "code-reviewer");
    assert_eq!(input.mentions[0].mention_type, MentionType::Agent);
}

// --- Multiple mentions ---

#[test]
fn test_multiple_mentions() {
    let input = process_user_input("Compare @src/a.rs and @src/b.rs#L5-10");
    assert_eq!(input.mentions.len(), 2);
    assert_eq!(input.mentions[0].text, "src/a.rs");
    assert_eq!(input.mentions[0].mention_type, MentionType::FilePath);
    assert_eq!(input.mentions[1].text, "src/b.rs");
    assert_eq!(input.mentions[1].line_start, Some(5));
    assert_eq!(input.mentions[1].line_end, Some(10));
}

// --- Edge cases ---

#[test]
fn test_at_sign_at_end() {
    let input = process_user_input("email@");
    // @ not preceded by whitespace → not a mention
    assert_eq!(input.mentions.len(), 0);
}

#[test]
fn test_at_sign_mid_word() {
    let input = process_user_input("user@example.com");
    // @ not preceded by whitespace → not a mention
    assert_eq!(input.mentions.len(), 0);
}

#[test]
fn test_unclosed_quote() {
    let input = process_user_input("Look at @\"unclosed file");
    // Unclosed quote → malformed mention is skipped
    assert_eq!(input.mentions.len(), 0);
}

#[test]
fn test_empty_quotes() {
    let input = process_user_input("Look at @\"\"");
    // Empty quoted mention → skipped
    assert_eq!(input.mentions.len(), 0);
}

#[test]
fn test_symbol_mention() {
    let input = process_user_input("Find @#MyStruct");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "#MyStruct");
    assert_eq!(input.mentions[0].mention_type, MentionType::Symbol);
}

// --- parse_line_range unit tests ---

#[test]
fn test_parse_line_range_none() {
    let (path, start, end) = parse_line_range("src/main.rs");
    assert_eq!(path, "src/main.rs");
    assert_eq!(start, None);
    assert_eq!(end, None);
}

#[test]
fn test_parse_line_range_single() {
    let (path, start, end) = parse_line_range("src/main.rs#L42");
    assert_eq!(path, "src/main.rs");
    assert_eq!(start, Some(42));
    // lineEnd defaults to lineStart for `#L<n>` alone.
    assert_eq!(end, Some(42));
}

#[test]
fn test_parse_line_range_strips_heading() {
    let (path, start, end) = parse_line_range("src/main.rs#some-heading");
    assert_eq!(path, "src/main.rs");
    assert_eq!(start, None);
    assert_eq!(end, None);
}

#[test]
fn test_mcp_resource_mention() {
    let input = process_user_input("Read @server1:resource/path");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].text, "server1:resource/path");
    match &input.mentions[0].mention_type {
        MentionType::McpResource { server, uri } => {
            assert_eq!(server, "server1");
            assert_eq!(uri, "resource/path");
        }
        other => panic!("Expected McpResource, got {other:?}"),
    }
}

#[test]
fn test_mcp_resource_not_confused_with_url() {
    let input = process_user_input("Open @https://example.com");
    assert_eq!(input.mentions.len(), 1);
    assert_eq!(input.mentions[0].mention_type, MentionType::Url);
}

#[test]
fn test_mcp_resource_not_confused_with_path() {
    // `src/main.rs` has no colon → still FilePath.
    let input = process_user_input("Read @src/main.rs");
    assert_eq!(input.mentions[0].mention_type, MentionType::FilePath);
}

#[test]
fn test_parse_line_range_span() {
    let (path, start, end) = parse_line_range("src/main.rs#L10-50");
    assert_eq!(path, "src/main.rs");
    assert_eq!(start, Some(10));
    assert_eq!(end, Some(50));
}
