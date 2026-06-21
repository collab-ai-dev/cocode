use super::*;
use coco_messages::Message;
use coco_messages::StopReason;

fn api_error_text(msg: &Message) -> String {
    let Message::Assistant(asst) = msg else {
        panic!("expected Assistant message, got {msg:?}");
    };
    asst.api_error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default()
}

#[test]
fn api_error_message_max_tokens_includes_configured_cap() {
    let msg = build_abnormal_stop_api_error_message(StopReason::MaxTokens, Some(8_000));
    let text = api_error_text(&msg);
    assert!(
        text.contains("8000"),
        "configured max_tokens must appear in user-facing text: {text}"
    );
    assert!(text.starts_with("API Error"));
}

#[test]
fn api_error_message_max_tokens_without_cap_falls_back() {
    let msg = build_abnormal_stop_api_error_message(StopReason::MaxTokens, None);
    let text = api_error_text(&msg);
    assert!(text.contains("output token maximum"));
}

#[test]
fn api_error_message_context_window_exceeded_is_distinct_variant() {
    let msg = build_abnormal_stop_api_error_message(StopReason::ContextWindowExceeded, Some(8_000));
    let text = api_error_text(&msg);
    assert!(
        text.contains("context window"),
        "context-window-exceeded must distinguish from plain max_tokens: {text}"
    );
    assert!(
        !text.contains("8000"),
        "context-window message must not show output-token cap (would mislead the user about which limit was hit): {text}"
    );
}

#[test]
fn api_error_message_content_filter_is_provider_agnostic() {
    let msg = build_abnormal_stop_api_error_message(StopReason::ContentFilter, None);
    let text = api_error_text(&msg);
    assert!(text.contains("declined"));
    assert!(
        text.contains("content policy") || text.contains("safety"),
        "must mention the policy/safety bucket so it covers refusal / SAFETY / RECITATION: {text}"
    );
    // Provider-agnostic — no "Claude" / "Anthropic" / "OpenAI" mentions.
    assert!(!text.contains("Claude"));
    assert!(!text.contains("Anthropic"));
}

#[test]
fn api_error_message_has_empty_content_carry() {
    // The synthetic message body is empty content + api_error field.
    // The real partial assistant message is pushed separately by the
    // engine; this one is purely the typed signal.
    let msg = build_abnormal_stop_api_error_message(StopReason::ContentFilter, None);
    let Message::Assistant(asst) = &msg else {
        panic!("expected Assistant message");
    };
    let coco_llm_types::LlmMessage::Assistant { content, .. } = &asst.message else {
        panic!("expected Assistant LlmMessage variant");
    };
    assert!(
        content.is_empty(),
        "synthetic message must have empty content — the real partial \
         response is pushed separately by the engine"
    );
    assert!(asst.api_error.is_some(), "api_error field must be set");
}

#[test]
fn api_error_message_unspecified_variant_uses_wire_str() {
    // `Error` / `Other` shouldn't normally reach this builder (engine
    // only invokes it for ContentFilter / MaxTokens / ContextWindowExceeded),
    // but if it does the fallback should not panic and should
    // surface the typed variant by its wire name.
    let msg = build_abnormal_stop_api_error_message(StopReason::Error, None);
    let text = api_error_text(&msg);
    assert!(
        text.contains("error"),
        "fallback should name the variant: {text}"
    );
}

#[tokio::test]
async fn drain_command_queue_into_history_leaves_slash_commands_queued() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new(
            "  /compact foo".into(),
            QueuePriority::Next,
        ))
        .await;
    queue
        .enqueue(QueuedCommand::new("continue".into(), QueuePriority::Next))
        .await;
    let mut history = coco_messages::MessageHistory::new();

    drain_command_queue_into_history(&queue, &mut history, &None, QueuePriority::Later, None).await;

    assert_eq!(history.len(), 1);
    assert_eq!(queue.len().await, 1);
    let remaining = queue
        .dequeue_first_matching(|c| c.is_slash_command)
        .await
        .expect("slash command should remain queued");
    assert_eq!(remaining.prompt, "  /compact foo");
}

// ── Steering: human queued commands become raw user messages; the
// model-facing wrapper is applied only at prompt-build (mirrors TS).

fn msg_text(m: &Message) -> String {
    coco_messages::wrapping::extract_text_from_message(m)
}

#[test]
fn queued_command_to_message_human_is_raw_user_message() {
    let cmd =
        QueuedCommand::new("plan it".into(), QueuePriority::Next).with_origin(QueueOrigin::Human);
    let msg = queued_command_to_message(&cmd);
    let Message::User(u) = &msg else {
        panic!("human steering must be a User message, got {msg:?}");
    };
    assert_eq!(u.origin, Some(MessageOrigin::QueuedSteering));
    let text = msg_text(&msg);
    assert_eq!(text, "plan it");
    assert!(
        !text.contains("<system-reminder>"),
        "history copy must be raw"
    );
}

#[test]
fn queued_command_to_message_none_origin_is_raw_user_message() {
    // No explicit origin == human-typed; still a raw steering user message.
    let cmd = QueuedCommand::new("hi".into(), QueuePriority::Next);
    assert!(matches!(
        queued_command_to_message(&cmd),
        Message::User(u) if u.origin == Some(MessageOrigin::QueuedSteering)
    ));
}

#[test]
fn queued_command_to_message_coordinator_stays_model_only_attachment() {
    let cmd = QueuedCommand::new("ping".into(), QueuePriority::Next)
        .with_origin(QueueOrigin::Coordinator);
    let msg = queued_command_to_message(&cmd);
    let Message::Attachment(att) = &msg else {
        panic!("coordinator must stay an attachment, got {msg:?}");
    };
    assert_eq!(att.kind, AttachmentKind::QueuedCommand);
}

#[test]
fn wrap_steering_messages_for_api_wraps_only_steering() {
    let steer = std::sync::Arc::new(queued_command_to_message(
        &QueuedCommand::new("do the thing".into(), QueuePriority::Next)
            .with_origin(QueueOrigin::Human),
    ));
    let normal = std::sync::Arc::new(coco_messages::create_user_message("regular input"));
    let wrapped = wrap_steering_messages_for_api(&[steer, normal]);

    let steer_text = msg_text(wrapped[0].as_ref());
    assert!(
        steer_text.contains("<system-reminder>")
            && steer_text.contains("The user sent a new message while you were working:")
            && steer_text.contains("do the thing"),
        "steering message must be wrapped for the API: {steer_text}"
    );

    let normal_text = msg_text(wrapped[1].as_ref());
    assert_eq!(
        normal_text, "regular input",
        "non-steering message must be left untouched"
    );
}
