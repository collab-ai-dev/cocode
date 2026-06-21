use super::*;
use coco_types::messages::Message;
use pretty_assertions::assert_eq;

fn user_text(msg: &Message) -> (&str, bool) {
    let Message::User(u) = msg else {
        panic!("expected user message");
    };
    let coco_types::messages::LlmMessage::User { content, .. } = &u.message else {
        panic!("expected llm user");
    };
    let coco_types::messages::UserContent::Text(t) = &content[0] else {
        panic!("expected text part");
    };
    (t.text.as_str(), u.is_visible_in_transcript_only)
}

#[test]
fn test_format_command_input_round_trips_via_extract_tag() {
    let body = format_command_input("model", "sonnet");
    assert_eq!(extract_tag(&body, COMMAND_NAME_TAG), Some("/model"));
    assert_eq!(extract_tag(&body, COMMAND_MESSAGE_TAG), Some("model"));
    assert_eq!(extract_tag(&body, COMMAND_ARGS_TAG), Some("sonnet"));
    assert!(is_command_input(&body));
    assert!(!is_local_command_output(&body));
}

#[test]
fn test_format_local_command_stdout_empty_uses_no_content() {
    let body = format_local_command_stdout("");
    assert_eq!(
        extract_tag(&body, LOCAL_COMMAND_STDOUT_TAG),
        Some(NO_CONTENT_MESSAGE)
    );
    assert!(is_local_command_output(&body));
}

#[test]
fn test_build_is_transcript_only_and_carries_args() {
    // Tool/config commands (/help, /model, …) render `❯ /cmd args` + `⎿ out`
    // but are transcript-only — the LLM never sees them.
    let msgs = build_slash_command_messages("help", "patterns", "the help text", false);
    assert_eq!(msgs.len(), 2);
    let (echo, echo_t_only) = user_text(&msgs[0]);
    let (result, result_t_only) = user_text(&msgs[1]);
    assert!(is_command_input(echo));
    assert!(is_local_command_output(result));
    assert_eq!(extract_tag(echo, COMMAND_ARGS_TAG), Some("patterns"));
    // Neither reaches the model.
    assert!(echo_t_only);
    assert!(result_t_only);
}

#[test]
fn test_build_context_usage_messages_echo_then_system_snapshot() {
    // `/context` prints inline: a `❯ /context` echo (transcript-only User)
    // followed by a typed System snapshot the TUI paints. Neither is a
    // `<local-command-stdout>` body — the second message is a System, not a
    // markdown User result.
    let result = coco_types::ContextUsageResult {
        total_tokens: 1234,
        max_tokens: 200_000,
        raw_max_tokens: 200_000,
        percentage: 0.6,
        model: "anthropic/claude".to_string(),
        categories: Vec::new(),
        is_auto_compact_enabled: false,
        auto_compact_threshold: None,
        message_breakdown: None,
        memory_files: Vec::new(),
        mcp_tools: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        suggestions: Vec::new(),
    };
    let msgs = build_context_usage_messages("", result);
    assert_eq!(msgs.len(), 2);

    let (echo, echo_t_only) = user_text(&msgs[0]);
    assert!(is_command_input(echo));
    assert_eq!(extract_tag(echo, COMMAND_NAME_TAG), Some("/context"));
    assert!(echo_t_only, "echo must never reach the model");

    let coco_types::messages::Message::System(coco_types::messages::SystemMessage::ContextUsage(
        snapshot,
    )) = &msgs[1]
    else {
        panic!("expected System(ContextUsage) snapshot");
    };
    assert_eq!(snapshot.result.total_tokens, 1234);
}

#[test]
fn test_sensitive_args_redacted() {
    let msgs =
        build_slash_command_messages("login", "secret-token", "ok", /*is_sensitive*/ true);
    let (echo, _) = user_text(&msgs[0]);
    assert_eq!(extract_tag(echo, COMMAND_ARGS_TAG), Some("***"));
}

#[test]
fn test_slash_user_message_can_be_model_visible() {
    // Documents the escape hatch for the `display: 'user'` flag: flip
    // `transcript_only` to false to make a slash echo/result model-visible.
    let hidden = slash_user_message("x", /*transcript_only*/ true);
    let visible = slash_user_message("x", /*transcript_only*/ false);
    let (_, hidden_t_only) = user_text(&hidden);
    let (_, visible_t_only) = user_text(&visible);
    assert!(hidden_t_only);
    assert!(!visible_t_only);
}
