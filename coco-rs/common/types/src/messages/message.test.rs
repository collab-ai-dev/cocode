use super::*;

#[test]
fn test_message_kind() {
    let msg = Message::Tombstone(TombstoneMessage {
        uuid: Uuid::new_v4(),
        original_kind: MessageKind::User,
    });
    assert_eq!(msg.kind(), MessageKind::Tombstone);
}

#[test]
fn test_stop_reason_serde() {
    let reason = StopReason::EndTurn;
    let json = serde_json::to_string(&reason).unwrap();
    assert_eq!(json, "\"end_turn\"");
}

#[test]
fn test_system_message_level_serde() {
    let level = SystemMessageLevel::Warning;
    let json = serde_json::to_string(&level).unwrap();
    assert_eq!(json, "\"warning\"");
}

fn user_msg(transcript_only: bool) -> Message {
    Message::User(UserMessage {
        message: crate::LlmMessage::user_text("hi"),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: transcript_only,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

#[test]
fn test_transcript_only_user_is_ui_only_not_api() {
    // The model-visibility gate: a transcript-only user message (e.g. a
    // slash-command echo/result with `display: system`) renders but is
    // never sent to the model.
    let v = user_msg(/*transcript_only*/ true).visibility();
    assert!(v.ui, "transcript-only message must still render");
    assert!(!v.api, "transcript-only message must NOT reach the model");
}

#[test]
fn test_normal_user_is_both_visible() {
    let v = user_msg(/*transcript_only*/ false).visibility();
    assert!(v.ui);
    assert!(v.api);
}

#[test]
fn test_compact_summary_stays_api_visible_despite_transcript_only() {
    // Regression: a compaction summary carries BOTH
    // `is_visible_in_transcript_only` (it replaces the summarized history in
    // the transcript) AND `is_compact_summary`. It MUST reach the model — it
    // is the post-compact context. The transcript-only UI_ONLY arm must
    // exempt compact summaries, otherwise every compaction silently drops the
    // summary from the prompt.
    let msg = Message::User(UserMessage {
        message: crate::LlmMessage::user_text("summary of the prior conversation"),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: true,
        is_virtual: false,
        is_compact_summary: true,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    });
    let v = msg.visibility();
    assert!(v.api, "compaction summary must reach the model");
    assert!(
        v.ui,
        "compaction summary must also render in the transcript"
    );
}

#[test]
fn test_message_origin_slash_command_serde() {
    let json = serde_json::to_string(&MessageOrigin::SlashCommand).unwrap();
    assert_eq!(json, "\"slash_command\"");
}

#[test]
fn mention_summary_attachment_is_display_only() {
    let att = AttachmentMessage::mention_summary(crate::MentionSummaryPayload {
        items: vec![crate::MentionSummaryItem {
            display_path: "src/lib.rs".to_string(),
            kind: crate::MentionItemKind::File,
            count: Some(42),
            truncated: false,
        }],
    });

    assert_eq!(att.kind, AttachmentKind::File);
    // `Unit` body → dropped from the API request (no empty message to model).
    assert!(att.as_api_message().is_none());
    assert!(att.as_text_for_display().is_empty());
    // Typed extras carry the rows; `File` keeps the cell renderable.
    assert!(matches!(
        att.extras,
        Some(AttachmentExtras::MentionSummary(_))
    ));
    assert!(att.kind.renders_in_transcript());
}
