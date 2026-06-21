use crate::*;
use uuid::Uuid;

use super::ensure_user_first;
use super::merge_consecutive_assistant_messages;
use super::merge_consecutive_user_messages;
use super::normalize_messages_for_api;
use super::strip_images_from_messages;
use super::strip_signature_blocks;
use super::to_llm_prompt;

/// Test helper: wrap a borrowed `&[Message]` (or `Vec<Message>` via deref)
/// into the canonical `Vec<Arc<Message>>` shape that the post-refactor
/// `normalize_messages_for_api` accepts. Caller keeps ownership.
fn arc_vec(msgs: &[Message]) -> Vec<std::sync::Arc<Message>> {
    msgs.iter().cloned().map(std::sync::Arc::new).collect()
}

fn user_msg(text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn virtual_msg(text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: true,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn assistant_msg(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: text.into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn assistant_api_error(message: &str, error_type: Option<&str>) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![]),
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: Some(ApiError {
            message: message.to_string(),
            status_code: None,
            error_type: error_type.map(str::to_string),
        }),
    })
}

fn user_with_files() -> Message {
    Message::User(UserMessage {
        message: LlmMessage::User {
            content: vec![
                UserContent::text("look"),
                UserContent::image(vec![1, 2, 3], "image/png"),
                UserContent::File(FileContent::from_base64("JVBERi0=", "application/pdf")),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn tombstone_msg() -> Message {
    Message::Tombstone(TombstoneMessage {
        uuid: Uuid::new_v4(),
        original_kind: MessageKind::User,
    })
}

#[test]
fn test_filters_virtual_messages() {
    let msgs = vec![user_msg("hello"), virtual_msg("ghost"), assistant_msg("hi")];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    assert_eq!(result.len(), 2); // virtual filtered out
}

#[test]
fn test_filters_tombstones() {
    let msgs = vec![user_msg("hello"), tombstone_msg(), assistant_msg("hi")];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    assert_eq!(result.len(), 2); // tombstone filtered out
}

#[test]
fn api_error_system_message_is_dropped_from_api_request() {
    // The inline turn-failure row (`create_api_error_message`) is display-only
    // transcript provenance — it must never re-enter the model context.
    let msgs = vec![
        user_msg("hello"),
        crate::create_api_error_message("LLM stream failed: error decoding response body", None),
        assistant_msg("hi"),
    ];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    assert_eq!(result.len(), 2, "ApiError system message filtered out");
}

#[test]
fn normalize_strips_prior_image_after_image_too_large_error() {
    let msgs = vec![
        user_with_files(),
        assistant_api_error("An image in the conversation is too large", None),
        user_with_files(),
    ];

    let result = normalize_messages_for_api(&arc_vec(&msgs));

    let LlmMessage::User { content, .. } = &result[0] else {
        panic!("expected first user message");
    };
    assert_eq!(
        content
            .iter()
            .filter(|part| matches!(
                part,
                UserContent::File(file) if file.media_type == "image/png"
            ))
            .count(),
        1,
        "only the new post-error image should remain after user merge"
    );
    assert!(content.iter().any(|part| matches!(
        part,
        UserContent::File(file) if file.media_type == "application/pdf"
    )));

    let LlmMessage::User { content, .. } = result.last().unwrap() else {
        panic!("expected last user message");
    };
    assert!(content.iter().any(|part| matches!(
        part,
        UserContent::File(file) if file.media_type == "image/png"
    )));
}

#[test]
fn normalize_strips_prior_document_after_pdf_too_large_error() {
    let msgs = vec![
        user_with_files(),
        assistant_api_error("PDF document is too large to send", None),
    ];

    let result = normalize_messages_for_api(&arc_vec(&msgs));

    let LlmMessage::User { content, .. } = &result[0] else {
        panic!("expected user message");
    };
    assert!(content.iter().any(|part| matches!(
        part,
        UserContent::File(file) if file.media_type == "image/png"
    )));
    assert!(!content.iter().any(|part| matches!(
        part,
        UserContent::File(file) if file.media_type == "application/pdf"
    )));
}

#[test]
fn normalize_strips_prior_image_and_document_after_request_too_large_error() {
    let msgs = vec![
        user_with_files(),
        assistant_api_error("Request too large", Some("request_too_large")),
    ];

    let result = normalize_messages_for_api(&arc_vec(&msgs));

    let LlmMessage::User { content, .. } = &result[0] else {
        panic!("expected user message");
    };
    assert_eq!(content.len(), 1);
    assert!(matches!(&content[0], UserContent::Text(_)));
}

#[test]
fn test_merges_consecutive_user_messages() {
    let msgs = vec![user_msg("hello"), user_msg("world"), assistant_msg("hi")];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    // Two user messages merged into one, plus assistant = 2
    assert_eq!(result.len(), 2);
}

#[test]
fn test_ensures_starts_with_user() {
    let msgs = vec![assistant_msg("hi")];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    assert!(matches!(&result[0], LlmMessage::User { .. }));
}

#[test]
fn test_empty_input() {
    let empty: Vec<Message> = Vec::new();
    let result = normalize_messages_for_api(&arc_vec(&empty));
    assert!(result.is_empty());
}

#[test]
fn test_progress_messages_filtered() {
    let msgs = vec![
        user_msg("hello"),
        Message::Progress(ProgressMessage {
            tool_use_id: "test".into(),
            data: serde_json::Value::Null,
            parent_message_uuid: None,
        }),
        assistant_msg("hi"),
    ];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    assert_eq!(result.len(), 2); // progress filtered
}

// === merge_consecutive_user_messages ===

#[test]
fn test_merge_consecutive_user_messages_basic() {
    let mut msgs = vec![user_msg("hello"), user_msg("world"), assistant_msg("hi")];
    merge_consecutive_user_messages(&mut msgs);
    assert_eq!(msgs.len(), 2);
    // First message should have merged content.
    if let Message::User(u) = &msgs[0] {
        if let LlmMessage::User { content, .. } = &u.message {
            assert_eq!(content.len(), 2); // two text parts merged
        } else {
            panic!("expected User LlmMessage");
        }
    } else {
        panic!("expected User message");
    }
}

#[test]
fn test_merge_consecutive_user_messages_inserts_seam_newline() {
    // #83: merging two text user messages inserts a newline at the seam
    // so the Anthropic API doesn't concatenate them ('2+2' + '3+3').
    let mut msgs = vec![user_msg("2+2"), user_msg("3+3")];
    merge_consecutive_user_messages(&mut msgs);
    let Message::User(u) = &msgs[0] else {
        panic!("expected User")
    };
    let LlmMessage::User { content, .. } = &u.message else {
        panic!("expected User Llm")
    };
    let crate::UserContent::Text(first) = &content[0] else {
        panic!("expected text part")
    };
    assert_eq!(
        first.text, "2+2\n",
        "seam newline must be appended to the first part"
    );
}

#[test]
fn test_merge_consecutive_user_messages_empty() {
    let mut msgs: Vec<Message> = vec![];
    merge_consecutive_user_messages(&mut msgs);
    assert!(msgs.is_empty());
}

#[test]
fn test_merge_consecutive_user_messages_single() {
    let mut msgs = vec![user_msg("only")];
    merge_consecutive_user_messages(&mut msgs);
    assert_eq!(msgs.len(), 1);
}

#[test]
fn test_merge_consecutive_user_messages_no_merge() {
    let mut msgs = vec![user_msg("a"), assistant_msg("b"), user_msg("c")];
    merge_consecutive_user_messages(&mut msgs);
    assert_eq!(msgs.len(), 3);
}

#[test]
fn test_merge_consecutive_user_messages_three() {
    let mut msgs = vec![user_msg("a"), user_msg("b"), user_msg("c")];
    merge_consecutive_user_messages(&mut msgs);
    assert_eq!(msgs.len(), 1);
    if let Message::User(u) = &msgs[0] {
        if let LlmMessage::User { content, .. } = &u.message {
            assert_eq!(content.len(), 3);
        } else {
            panic!("expected User LlmMessage");
        }
    } else {
        panic!("expected User message");
    }
}

// === merge_consecutive_assistant_messages ===

#[test]
fn test_merge_consecutive_assistant_messages_basic() {
    let mut msgs = vec![user_msg("hi"), assistant_msg("a"), assistant_msg("b")];
    merge_consecutive_assistant_messages(&mut msgs);
    assert_eq!(msgs.len(), 2);
    if let Message::Assistant(a) = &msgs[1] {
        if let LlmMessage::Assistant { content, .. } = &a.message {
            assert_eq!(content.len(), 2);
        } else {
            panic!("expected Assistant LlmMessage");
        }
    } else {
        panic!("expected Assistant message");
    }
}

#[test]
fn test_merge_consecutive_assistant_messages_empty() {
    let mut msgs: Vec<Message> = vec![];
    merge_consecutive_assistant_messages(&mut msgs);
    assert!(msgs.is_empty());
}

#[test]
fn test_merge_consecutive_assistant_messages_no_merge() {
    let mut msgs = vec![assistant_msg("a"), user_msg("b"), assistant_msg("c")];
    merge_consecutive_assistant_messages(&mut msgs);
    assert_eq!(msgs.len(), 3);
}

// === strip_images_from_messages ===

fn user_msg_with_image() -> Message {
    use crate::UserContent;
    use coco_llm_types::FilePart;
    use coco_llm_types::TextPart;

    Message::User(UserMessage {
        message: LlmMessage::User {
            content: vec![
                UserContent::Text(TextPart::new("caption")),
                UserContent::File(FilePart::from_base64("abc123", "image/png")),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn user_msg_image_only() -> Message {
    use crate::UserContent;
    use coco_llm_types::FilePart;

    Message::User(UserMessage {
        message: LlmMessage::User {
            content: vec![UserContent::File(FilePart::from_base64(
                "abc123",
                "image/jpeg",
            ))],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

#[test]
fn test_strip_images_keeps_text() {
    let mut msgs = vec![user_msg_with_image()];
    strip_images_from_messages(&mut msgs);
    assert_eq!(msgs.len(), 1);
    if let Message::User(u) = &msgs[0] {
        if let LlmMessage::User { content, .. } = &u.message {
            assert_eq!(content.len(), 1);
            assert!(matches!(content[0], crate::UserContent::Text(_)));
        } else {
            panic!("expected User LlmMessage");
        }
    } else {
        panic!("expected User message");
    }
}

#[test]
fn test_strip_images_removes_empty_messages() {
    let mut msgs = vec![user_msg("keep"), user_msg_image_only()];
    strip_images_from_messages(&mut msgs);
    assert_eq!(msgs.len(), 1);
}

#[test]
fn test_strip_images_empty_input() {
    let mut msgs: Vec<Message> = vec![];
    strip_images_from_messages(&mut msgs);
    assert!(msgs.is_empty());
}

#[test]
fn test_strip_images_preserves_assistant() {
    let mut msgs = vec![user_msg_with_image(), assistant_msg("hi")];
    strip_images_from_messages(&mut msgs);
    assert_eq!(msgs.len(), 2);
}

// === strip_signature_blocks ===

fn user_msg_with_sig(text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

#[test]
fn test_strip_signature_basic() {
    let mut msgs = vec![user_msg_with_sig("Hello\n-- \nJohn Doe")];
    strip_signature_blocks(&mut msgs);
    if let Message::User(u) = &msgs[0] {
        if let LlmMessage::User { content, .. } = &u.message {
            if let crate::UserContent::Text(t) = &content[0] {
                assert_eq!(t.text, "Hello");
            } else {
                panic!("expected text");
            }
        } else {
            panic!("expected user llm msg");
        }
    } else {
        panic!("expected user msg");
    }
}

#[test]
fn test_strip_signature_no_sig() {
    let mut msgs = vec![user_msg_with_sig("No signature here")];
    strip_signature_blocks(&mut msgs);
    if let Message::User(u) = &msgs[0] {
        if let LlmMessage::User { content, .. } = &u.message {
            if let crate::UserContent::Text(t) = &content[0] {
                assert_eq!(t.text, "No signature here");
            } else {
                panic!("expected text");
            }
        } else {
            panic!("expected user llm msg");
        }
    } else {
        panic!("expected user msg");
    }
}

#[test]
fn test_strip_signature_empty() {
    let mut msgs: Vec<Message> = vec![];
    strip_signature_blocks(&mut msgs);
    assert!(msgs.is_empty());
}

// === ensure_user_first ===

#[test]
fn test_ensure_user_first_already_user() {
    let mut msgs = vec![user_msg("hello"), assistant_msg("hi")];
    ensure_user_first(&mut msgs);
    assert_eq!(msgs.len(), 2);
}

#[test]
fn test_ensure_user_first_prepends() {
    let mut msgs = vec![assistant_msg("hi")];
    ensure_user_first(&mut msgs);
    assert_eq!(msgs.len(), 2);
    assert!(matches!(msgs[0], Message::User(_)));
}

#[test]
fn test_ensure_user_first_empty() {
    let mut msgs: Vec<Message> = vec![];
    ensure_user_first(&mut msgs);
    assert!(msgs.is_empty());
}

// === to_llm_prompt ===

#[test]
fn test_to_llm_prompt_basic() {
    let msgs = vec![user_msg("hello"), assistant_msg("hi")];
    let prompt = to_llm_prompt(&msgs);
    assert_eq!(prompt.len(), 2);
    assert!(matches!(prompt[0], LlmMessage::User { .. }));
    assert!(matches!(prompt[1], LlmMessage::Assistant { .. }));
}

#[test]
fn test_to_llm_prompt_skips_system_and_progress() {
    let msgs = vec![
        user_msg("hello"),
        Message::Progress(ProgressMessage {
            tool_use_id: "test".into(),
            data: serde_json::Value::Null,
            parent_message_uuid: None,
        }),
        tombstone_msg(),
        assistant_msg("hi"),
    ];
    let prompt = to_llm_prompt(&msgs);
    // Progress, Tombstone are skipped.
    assert_eq!(prompt.len(), 2);
}

#[test]
fn test_to_llm_prompt_empty() {
    let prompt = to_llm_prompt(&[]);
    assert!(prompt.is_empty());
}

// ─────────────────────────────────────────────────────────────────────
// Regression tests for Round 10 deep-review fixes.
// ─────────────────────────────────────────────────────────────────────

fn assistant_msg_with_request_id(text: &str, request_id: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: text.into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: Some(request_id.into()),
        api_error: None,
    })
}

#[test]
fn merge_assistants_by_request_id_keeps_distinct_ids_separate() {
    // Different request_ids must stay separate.
    let msgs = vec![
        user_msg("hi"),
        assistant_msg_with_request_id("first", "req-A"),
        assistant_msg_with_request_id("second", "req-B"),
    ];
    let prompt = normalize_messages_for_api(&arc_vec(&msgs));
    // 1 user + 2 assistants (NOT merged, since request_ids differ).
    let asst_count = prompt
        .iter()
        .filter(|m| matches!(m, LlmMessage::Assistant { .. }))
        .count();
    assert_eq!(
        asst_count, 2,
        "different request_ids must NOT merge — TS only merges matching message.id"
    );
}

#[test]
fn merge_assistants_by_request_id_merges_matching_ids() {
    let msgs = vec![
        user_msg("hi"),
        assistant_msg_with_request_id("first", "req-A"),
        assistant_msg_with_request_id("second", "req-A"),
    ];
    let prompt = normalize_messages_for_api(&arc_vec(&msgs));
    let asst_count = prompt
        .iter()
        .filter(|m| matches!(m, LlmMessage::Assistant { .. }))
        .count();
    assert_eq!(
        asst_count, 1,
        "matching request_ids merge — TS streaming chunks"
    );
}

#[test]
fn merge_assistants_with_no_request_id_stays_separate() {
    // Synthetic / test messages with no request_id never merge.
    let msgs = vec![
        user_msg("hi"),
        assistant_msg("first"),  // request_id = None
        assistant_msg("second"), // request_id = None
    ];
    let prompt = normalize_messages_for_api(&arc_vec(&msgs));
    let asst_count = prompt
        .iter()
        .filter(|m| matches!(m, LlmMessage::Assistant { .. }))
        .count();
    assert_eq!(
        asst_count, 2,
        "request_id=None never matches itself — keeps assistants separate"
    );
}

#[test]
fn smoosh_folds_into_is_error_tool_result() {
    use coco_llm_types::ToolContentPart;
    use coco_llm_types::ToolResultContent;
    use coco_llm_types::ToolResultPart;

    // Construct a Tool LlmMessage with an is_error=true tool_result whose
    // output is a Content array. smooshIntoToolResult filters incoming
    // blocks to text-only and proceeds — Rust must do the same.
    let tool_msg = LlmMessage::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "tc1".into(),
            tool_name: "Bash".into(),
            output: ToolResultContent::Text {
                value: "error output".into(),
                provider_options: None,
            },
            is_error: true,
            provider_metadata: None,
        })],
        provider_options: None,
    };
    let user_sr = LlmMessage::User {
        content: vec![coco_llm_types::UserContentPart::text(
            "<system-reminder>\nctx\n</system-reminder>",
        )],
        provider_options: None,
    };

    let mut msgs = vec![tool_msg, user_sr];
    super::smoosh_system_reminder_into_tool_result(&mut msgs);

    assert_eq!(msgs.len(), 1, "SR-User must fold into is_error tool_result");
    let LlmMessage::Tool { content, .. } = &msgs[0] else {
        panic!("expected Tool LlmMessage");
    };
    let ToolContentPart::ToolResult(rp) = &content[0] else {
        panic!("expected ToolResult part");
    };
    let ToolResultContent::Text { value, .. } = &rp.output else {
        panic!("expected Text output");
    };
    assert!(
        value.contains("<system-reminder>"),
        "SR text must be folded into is_error tool_result"
    );
}

#[test]
fn sanitize_strips_non_text_from_is_error_tool_result() {
    use coco_llm_types::ToolContentPart;
    use coco_llm_types::ToolResultContent;
    use coco_llm_types::ToolResultContentPart;
    use coco_llm_types::ToolResultPart;

    // is_error=true with a mixed Content array (text + image). The image
    // part must be stripped; surviving texts join with \n\n.
    let mut msgs = vec![LlmMessage::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "tc1".into(),
            tool_name: "Bash".into(),
            output: ToolResultContent::Content {
                value: vec![
                    ToolResultContentPart::Text {
                        text: "stderr line 1".into(),
                        provider_options: None,
                    },
                    ToolResultContentPart::FileData {
                        data: "iVBORw0KGgo=".into(),
                        media_type: "image/png".into(),
                        filename: None,
                        provider_options: None,
                    },
                    ToolResultContentPart::Text {
                        text: "stderr line 2".into(),
                        provider_options: None,
                    },
                ],
                provider_options: None,
            },
            is_error: true,
            provider_metadata: None,
        })],
        provider_options: None,
    }];

    super::sanitize_error_tool_result_in_llm_messages(&mut msgs);

    let LlmMessage::Tool { content, .. } = &msgs[0] else {
        panic!("expected Tool LlmMessage");
    };
    let ToolContentPart::ToolResult(rp) = &content[0] else {
        panic!("expected ToolResult");
    };
    let ToolResultContent::Content { value, .. } = &rp.output else {
        panic!("expected Content variant");
    };
    assert_eq!(value.len(), 1, "joined into one Text part");
    let ToolResultContentPart::Text { text, .. } = &value[0] else {
        panic!("expected Text after sanitize");
    };
    assert_eq!(text, "stderr line 1\n\nstderr line 2");
}

/// When an assistant tool_use has no matching tool_result anywhere
/// in the transcript, normalize_messages_for_api must inject an
/// `is_error: true` placeholder so the next provider call doesn't
/// hit `unexpected tool_use_id`.
#[test]
fn normalize_synthesizes_missing_tool_result() {
    use coco_llm_types::ToolCallPart;
    use coco_llm_types::ToolContentPart;
    use coco_llm_types::ToolResultContent;
    use coco_types::ToolId;
    use coco_types::ToolName;

    // Assistant emits tc1 + tc2; only tc1 has a result.
    let assistant = Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![
                AssistantContent::Text(coco_llm_types::TextPart {
                    text: "calling".into(),
                    provider_metadata: None,
                }),
                AssistantContent::ToolCall(ToolCallPart::new(
                    "tc1",
                    "Bash",
                    serde_json::json!({"command": "echo a"}),
                )),
                AssistantContent::ToolCall(ToolCallPart::new(
                    "tc2",
                    "Bash",
                    serde_json::json!({"command": "echo b"}),
                )),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    });
    let tool_result_tc1 = Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContentPart::ToolResult(
                coco_llm_types::ToolResultPart {
                    tool_call_id: "tc1".into(),
                    tool_name: "Bash".into(),
                    output: ToolResultContent::text("a"),
                    is_error: false,
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        tool_use_id: "tc1".into(),
        tool_id: ToolId::Builtin(ToolName::Bash),
        is_error: false,
    });

    let result =
        normalize_messages_for_api(&arc_vec(&[user_msg("go"), assistant, tool_result_tc1]));

    // Find the Tool message — it must now carry BOTH tc1 (real) and
    // tc2 (synthesized is_error). Order: real first (it predates the
    // synthesizer), synthetic appended.
    let tool_idx = result
        .iter()
        .position(|m| matches!(m, LlmMessage::Tool { .. }))
        .expect("Tool message must exist");
    let LlmMessage::Tool { content, .. } = &result[tool_idx] else {
        unreachable!()
    };
    let ids: Vec<(String, bool)> = content
        .iter()
        .filter_map(|p| match p {
            ToolContentPart::ToolResult(r) => Some((r.tool_call_id.clone(), r.is_error)),
            _ => None,
        })
        .collect();
    assert!(
        ids.iter().any(|(id, e)| id == "tc1" && !*e),
        "tc1 real result must survive: got {ids:?}"
    );
    assert!(
        ids.iter().any(|(id, e)| id == "tc2" && *e),
        "tc2 synthetic is_error result must be injected: got {ids:?}"
    );
}

/// Synthesis must be idempotent — running it twice on the same Vec
/// must not produce duplicate tool_results. Calls the private
/// `synthesize_missing_tool_results` directly (twice) so this test
/// genuinely exercises the second-pass path, not just observes a
/// single normalize-call output.
#[test]
fn normalize_synthesis_is_idempotent() {
    use coco_llm_types::ToolCallPart;
    use coco_llm_types::ToolContentPart;

    // Build a Vec<LlmMessage> with one assistant tool_use and no
    // matching tool_result.
    let mut msgs = vec![
        LlmMessage::user_text("go"),
        LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(ToolCallPart::new(
                "orphan",
                "Bash",
                serde_json::json!({}),
            ))],
            provider_options: None,
        },
    ];

    super::synthesize_missing_tool_results(&mut msgs);
    let count_after_first = count_tool_results_for(&msgs, "orphan");
    assert_eq!(count_after_first, 1, "first pass must inject the synthetic");

    // Second call on the SAME mutated Vec — the synthetic from pass 1
    // is now in `resolved`, so pass 2 must be a no-op.
    super::synthesize_missing_tool_results(&mut msgs);
    let count_after_second = count_tool_results_for(&msgs, "orphan");
    assert_eq!(
        count_after_second, 1,
        "second pass must NOT duplicate the synthetic; got {count_after_second} parts"
    );

    fn count_tool_results_for(msgs: &[LlmMessage], id: &str) -> usize {
        msgs.iter()
            .map(|m| match m {
                LlmMessage::Tool { content, .. } => content
                    .iter()
                    .filter(|p| matches!(p, ToolContentPart::ToolResult(r) if r.tool_call_id == id))
                    .count(),
                _ => 0,
            })
            .sum()
    }
}

/// Two assistants with orphans, separated by a Tool message that
/// resolves a different tool_use. Each assistant's orphan must be
/// synthesized independently — verifies the audit's edge case A2.
#[test]
fn normalize_synthesizes_for_multiple_assistants() {
    use coco_llm_types::ToolCallPart;
    use coco_llm_types::ToolContentPart;
    use coco_types::ToolId;
    use coco_types::ToolName;

    let asst_a = Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(ToolCallPart::new(
                "tcA",
                "Bash",
                serde_json::json!({}),
            ))],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    });
    // Tool that resolves an unrelated id (decoy — must NOT be treated
    // as covering tcA or tcB).
    let unrelated = Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContentPart::ToolResult(
                coco_llm_types::ToolResultPart {
                    tool_call_id: "unrelated".into(),
                    tool_name: "Bash".into(),
                    output: coco_llm_types::ToolResultContent::text("ok"),
                    is_error: false,
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        tool_use_id: "unrelated".into(),
        tool_id: ToolId::Builtin(ToolName::Bash),
        is_error: false,
    });
    let asst_b = Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(ToolCallPart::new(
                "tcB",
                "Bash",
                serde_json::json!({}),
            ))],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    });

    let result = normalize_messages_for_api(&arc_vec(&[user_msg("go"), asst_a, unrelated, asst_b]));

    let collect_ids = |id: &str| {
        result
            .iter()
            .flat_map(|m| match m {
                LlmMessage::Tool { content, .. } => content
                    .iter()
                    .filter_map(|p| match p {
                        ToolContentPart::ToolResult(r) if r.tool_call_id == id => Some(r.is_error),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>()
    };

    let tca = collect_ids("tcA");
    let tcb = collect_ids("tcB");
    assert_eq!(
        tca,
        vec![true],
        "tcA must have exactly one synthetic is_error result"
    );
    assert_eq!(
        tcb,
        vec![true],
        "tcB must have exactly one synthetic is_error result"
    );
}

// ── strip_observable_tool_input_for_api ──

fn exit_plan_mode_assistant(input: serde_json::Value) -> Message {
    use coco_llm_types::ToolCallPart;
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(ToolCallPart::new(
                "exit_plan_1",
                coco_types::ToolName::ExitPlanMode.as_str(),
                input,
            ))],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn exit_plan_mode_input(result: &[LlmMessage]) -> serde_json::Value {
    result
        .iter()
        .find_map(|m| match m {
            LlmMessage::Assistant { content, .. } => content.iter().find_map(|c| match c {
                AssistantContent::ToolCall(tc)
                    if tc.tool_name == coco_types::ToolName::ExitPlanMode.as_str() =>
                {
                    Some(tc.input.clone())
                }
                _ => None,
            }),
            _ => None,
        })
        .expect("ExitPlanMode tool call must survive normalization")
}

#[test]
fn normalize_strips_injected_exit_plan_mode_fields_for_api() {
    // `plan` / `planFilePath` are injected locally but stripped before the
    // API call — the wire schema is empty.
    let assistant = exit_plan_mode_assistant(serde_json::json!({
        "plan": "## Plan\n- ship it",
        "planFilePath": "/tmp/plans/slug.md",
        "allowedPrompts": [],
    }));
    let result = normalize_messages_for_api(&arc_vec(&[user_msg("go"), assistant]));

    let input = exit_plan_mode_input(&result);
    assert_eq!(input.get("plan"), None, "plan must be stripped before API");
    assert_eq!(
        input.get("planFilePath"),
        None,
        "planFilePath must be stripped before API"
    );
    assert_eq!(
        input.get("allowedPrompts"),
        Some(&serde_json::json!([])),
        "non-injected fields must be preserved"
    );
}

#[test]
fn normalize_leaves_exit_plan_mode_without_injected_fields_untouched() {
    let assistant = exit_plan_mode_assistant(serde_json::json!({"allowedPrompts": []}));
    let result = normalize_messages_for_api(&arc_vec(&[user_msg("go"), assistant]));

    assert_eq!(
        exit_plan_mode_input(&result),
        serde_json::json!({"allowedPrompts": []})
    );
}

#[test]
fn normalize_does_not_strip_plan_field_from_other_tools() {
    use coco_llm_types::ToolCallPart;
    // A non-ExitPlanMode tool that happens to carry a `plan` key keeps it.
    let assistant = Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::ToolCall(ToolCallPart::new(
                "call_1",
                "Read",
                serde_json::json!({"plan": "not the exit tool", "file_path": "/tmp/a"}),
            ))],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    });
    let result = normalize_messages_for_api(&arc_vec(&[user_msg("go"), assistant]));

    let input = result
        .iter()
        .find_map(|m| match m {
            LlmMessage::Assistant { content, .. } => content.iter().find_map(|c| match c {
                AssistantContent::ToolCall(tc) if tc.tool_name == "Read" => Some(tc.input.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("Read tool call must survive");
    assert_eq!(
        input.get("plan"),
        Some(&serde_json::json!("not the exit tool"))
    );
}

// ── MessagePass trait contract (drift detection) ────────────────────
//
// The pipeline assumes: if `would_mutate` returns `false`, then `apply`
// is a no-op. Violating this is a silent-failure mode — the fast path
// would skip materialize when it shouldn't. The tests below run every
// normalize pass against:
//   (a) "clean" inputs (no trigger condition holds) → assert `would_mutate`
//       is false AND `apply` leaves the Vec unchanged
//   (b) "dirty" inputs (specific trigger holds) → assert `would_mutate`
//       is true (we don't assert apply DOES mutate, because some passes
//       are over-conservative — but we do assert the slow path runs).
//
// New pass impls MUST add a `clean` + `dirty` case here. Otherwise
// `would_mutate` can silently drift away from `apply` body without any
// existing test catching it.

mod pipeline_invariants {
    use super::*;
    use crate::normalize::passes;
    use crate::pipeline::MessagePass;

    fn arc_refs(msgs: &[Message]) -> Vec<&Message> {
        msgs.iter().collect()
    }

    /// Run a pass against "clean" input and assert the trait contract:
    /// `would_mutate` is `false` AND `apply` leaves the slice unchanged.
    ///
    /// `Message` does not implement `PartialEq` (`AssistantContent` /
    /// `LlmMessage` carry vercel-ai provider blobs that intentionally use
    /// `serde_json::Value`), so we compare via canonical JSON
    /// serialization — equivalent for our test purpose.
    fn assert_clean<P: MessagePass>(pass: P, clean: Vec<Message>) {
        let refs = arc_refs(&clean);
        assert!(
            !pass.would_mutate(&refs),
            "would_mutate should be false on clean input"
        );
        drop(refs);
        let before = serde_json::to_value(&clean).expect("clean serializable");
        let mut owned = clean;
        pass.apply(&mut owned);
        let after = serde_json::to_value(&owned).expect("owned serializable");
        assert_eq!(
            before, after,
            "apply must be a no-op when would_mutate returned false (pass contract violation)"
        );
    }

    /// Run a pass against "dirty" input and assert `would_mutate` is `true`.
    /// Apply behavior depends on the pass; we only enforce the predicate
    /// fires so the slow path is taken.
    fn assert_dirty<P: MessagePass>(pass: P, dirty: Vec<Message>) {
        let refs = arc_refs(&dirty);
        assert!(
            pass.would_mutate(&refs),
            "would_mutate should be true on dirty input"
        );
    }

    // Helpers tailored for each pass's trigger condition.

    fn asst_with_content(content: Vec<AssistantContent>) -> Message {
        Message::Assistant(AssistantMessage {
            message: LlmMessage::Assistant {
                content,
                provider_options: None,
            },
            uuid: Uuid::new_v4(),
            model: "test".into(),
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            cost_usd: None,
            request_id: None,
            api_error: None,
        })
    }

    fn asst_with_req_id(content: Vec<AssistantContent>, req_id: &str) -> Message {
        Message::Assistant(AssistantMessage {
            message: LlmMessage::Assistant {
                content,
                provider_options: None,
            },
            uuid: Uuid::new_v4(),
            model: "test".into(),
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            cost_usd: None,
            request_id: Some(req_id.to_string()),
            api_error: None,
        })
    }

    fn text_part(t: &str) -> AssistantContent {
        AssistantContent::Text(TextContent {
            text: t.into(),
            provider_metadata: None,
        })
    }

    fn reasoning_part(t: &str) -> AssistantContent {
        AssistantContent::Reasoning(coco_llm_types::ReasoningPart::new(t.to_string()))
    }

    fn exit_plan_tool_call(injected: bool) -> AssistantContent {
        let input = if injected {
            serde_json::json!({"plan": "the plan"})
        } else {
            serde_json::json!({})
        };
        AssistantContent::ToolCall(coco_llm_types::ToolCallPart {
            tool_call_id: "id1".into(),
            tool_name: coco_types::ToolName::ExitPlanMode.as_str().to_string(),
            input,
            provider_executed: None,
            invalid: false,
            invalid_reason: None,
            provider_metadata: None,
        })
    }

    // ── Pass 1: OrphanedThinkingOnly ────────────────────────────────

    #[test]
    fn orphaned_thinking_clean_is_no_op() {
        assert_clean(
            passes::OrphanedThinkingOnly,
            vec![
                user_msg("hi"),
                asst_with_content(vec![text_part("hi back")]),
            ],
        );
    }

    #[test]
    fn orphaned_thinking_dirty_triggers() {
        assert_dirty(
            passes::OrphanedThinkingOnly,
            vec![asst_with_content(vec![reasoning_part("just thinking")])],
        );
    }

    // ── Pass 2: TrailingThinking ────────────────────────────────────

    #[test]
    fn trailing_thinking_clean_is_no_op() {
        assert_clean(
            passes::TrailingThinking,
            vec![
                user_msg("hi"),
                asst_with_content(vec![reasoning_part("think"), text_part("answer")]),
            ],
        );
    }

    #[test]
    fn trailing_thinking_dirty_triggers() {
        assert_dirty(
            passes::TrailingThinking,
            vec![asst_with_content(vec![
                text_part("answer"),
                reasoning_part("trailing"),
            ])],
        );
    }

    // ── Pass 3: WhitespaceOnly ──────────────────────────────────────

    #[test]
    fn whitespace_only_clean_is_no_op() {
        assert_clean(
            passes::WhitespaceOnly,
            vec![user_msg("hi"), asst_with_content(vec![text_part("answer")])],
        );
    }

    #[test]
    fn whitespace_only_dirty_triggers() {
        assert_dirty(
            passes::WhitespaceOnly,
            vec![asst_with_content(vec![text_part("   \n\t  ")])],
        );
    }

    // ── Pass 4: EnsureNonEmptyContent ───────────────────────────────

    #[test]
    fn ensure_non_empty_clean_is_no_op() {
        assert_clean(
            passes::EnsureNonEmptyContent,
            vec![user_msg("hi"), asst_with_content(vec![text_part("yes")])],
        );
    }

    #[test]
    fn ensure_non_empty_dirty_triggers() {
        // Non-final assistant with empty content → triggers.
        assert_dirty(
            passes::EnsureNonEmptyContent,
            vec![user_msg("hi"), asst_with_content(vec![]), user_msg("again")],
        );
    }

    // ── Pass 5: MergeConsecutiveUsers ───────────────────────────────

    #[test]
    fn merge_users_clean_is_no_op() {
        assert_clean(
            passes::MergeConsecutiveUsers,
            vec![user_msg("a"), asst_with_content(vec![text_part("b")])],
        );
    }

    #[test]
    fn merge_users_dirty_triggers() {
        assert_dirty(
            passes::MergeConsecutiveUsers,
            vec![user_msg("a"), user_msg("b")],
        );
    }

    // ── Pass 6: MergeAssistantsByRequestId ──────────────────────────

    #[test]
    fn merge_assistants_by_req_id_clean_is_no_op() {
        // Two assistants but different request_id → no merge.
        assert_clean(
            passes::MergeAssistantsByRequestId,
            vec![
                user_msg("hi"),
                asst_with_req_id(vec![text_part("a")], "req-1"),
                asst_with_req_id(vec![text_part("b")], "req-2"),
            ],
        );
    }

    #[test]
    fn merge_assistants_by_req_id_dirty_triggers() {
        assert_dirty(
            passes::MergeAssistantsByRequestId,
            vec![
                asst_with_req_id(vec![text_part("a")], "req-X"),
                asst_with_req_id(vec![text_part("b")], "req-X"),
            ],
        );
    }

    // ── Pass 7: StripExitPlanModeInjectedFields ─────────────────────

    #[test]
    fn strip_exit_plan_mode_clean_is_no_op() {
        // ExitPlanMode tool_call but no injected fields.
        assert_clean(
            passes::StripExitPlanModeInjectedFields,
            vec![asst_with_content(vec![exit_plan_tool_call(false)])],
        );
    }

    #[test]
    fn strip_exit_plan_mode_dirty_triggers() {
        assert_dirty(
            passes::StripExitPlanModeInjectedFields,
            vec![asst_with_content(vec![exit_plan_tool_call(true)])],
        );
    }

    // ── Empty input is always a no-op ───────────────────────────────

    #[test]
    fn empty_input_no_pass_mutates() {
        let empty: Vec<&Message> = Vec::new();
        assert!(!passes::OrphanedThinkingOnly.would_mutate(&empty));
        assert!(!passes::TrailingThinking.would_mutate(&empty));
        assert!(!passes::WhitespaceOnly.would_mutate(&empty));
        assert!(!passes::EnsureNonEmptyContent.would_mutate(&empty));
        assert!(!passes::MergeConsecutiveUsers.would_mutate(&empty));
        assert!(!passes::MergeAssistantsByRequestId.would_mutate(&empty));
        assert!(!passes::StripExitPlanModeInjectedFields.would_mutate(&empty));
    }
}

#[test]
fn test_local_command_system_message_becomes_user() {
    // #82: local_command system messages must reach the API as user
    // messages so the model can reference prior command output.
    let cmd = Message::System(SystemMessage::LocalCommand(SystemLocalCommandMessage {
        uuid: Uuid::new_v4(),
        command: "ls".into(),
        output: "file1.rs\nfile2.rs".into(),
    }));
    let msgs = vec![user_msg("run ls"), cmd];
    let result = normalize_messages_for_api(&arc_vec(&msgs));
    let joined: String = result
        .iter()
        .filter_map(|m| match m {
            LlmMessage::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|p| match p {
                        crate::UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("file1.rs"),
        "local command output must reach the API as user text: {joined}"
    );
}

// === validate_images_for_api ===

#[test]
fn test_validate_images_for_api_rejects_oversized_image() {
    use crate::UserContent;
    use coco_llm_types::FilePart;
    use coco_llm_types::TextPart;
    let big = "a".repeat(11); // base64 length 11 > cap 10
    let prompt = vec![LlmMessage::User {
        content: vec![
            UserContent::Text(TextPart::new("caption")),
            UserContent::File(FilePart::from_base64(big, "image/png")),
        ],
        provider_options: None,
    }];
    let err = validate_images_for_api(&prompt, 10).expect_err("oversized image must be rejected");
    assert_eq!(err.base64_len, 11);
    assert_eq!(err.max_base64_len, 10);
    assert!(err.message().contains("resize"));
}

#[test]
fn test_validate_images_for_api_allows_small_image_and_non_image_files() {
    use crate::UserContent;
    use coco_llm_types::FilePart;
    let prompt = vec![LlmMessage::User {
        content: vec![
            // 3-char base64 image, under the 10-byte cap.
            UserContent::File(FilePart::from_base64("abc", "image/png")),
            // A large non-image file is not subject to the image cap.
            UserContent::File(FilePart::from_base64("x".repeat(100), "application/pdf")),
        ],
        provider_options: None,
    }];
    assert!(validate_images_for_api(&prompt, 10).is_ok());
}
