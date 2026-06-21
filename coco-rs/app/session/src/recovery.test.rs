use super::*;
use coco_messages::Message;
use serde_json::json;

#[test]
fn test_can_resume_nonexistent() {
    assert!(!can_resume_session(Path::new("/nonexistent/path.jsonl")));
}

#[test]
fn test_fork_conversation_relabels_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.jsonl");
    let dst = dir.path().join("dest.jsonl");
    // Entry with a session_id is relabeled; a non-JSON line passes through.
    std::fs::write(
        &src,
        "{\"session_id\":\"src-id\",\"uuid\":\"u1\"}\nnot json\n",
    )
    .unwrap();
    fork_conversation(&src, &dst, "dest-id").unwrap();
    assert!(dst.exists());
    let out = std::fs::read_to_string(&dst).unwrap();
    let first: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(first["session_id"], "dest-id");
    assert_eq!(first["uuid"], "u1", "non-session fields preserved");
    assert!(
        out.lines().any(|l| l == "not json"),
        "bad line passes through"
    );
}

// ---------------------------------------------------------------------------
// load_conversation_for_resume — the actual resume entry point.
// Each test writes a synthetic JSONL transcript and walks the function.
// ---------------------------------------------------------------------------

/// Build a JSONL line for a `user` transcript entry with plain-text content.
///
/// `parent` matches the `parent_uuid` linkage — every user message
/// after the first should chain to the prior assistant so the resume
/// walker reconstructs the full chain via DAG traversal.
fn user_line_with_parent(uuid: &str, parent: Option<&str>, text: &str) -> String {
    let mut entry = json!({
        "type": "user",
        "uuid": uuid,
        "session_id": "s1",
        "cwd": "/tmp/p",
        "timestamp": "2025-01-15T10:00:00Z",
        "is_sidechain": false,
        "message": { "role": "user", "content": [{"type": "text", "text": text}] },
    });
    if let Some(p) = parent {
        entry["parent_uuid"] = json!(p);
    }
    serde_json::to_string(&entry).unwrap()
}

/// Convenience for the common "first user message in a session" case.
fn user_line(uuid: &str, text: &str) -> String {
    user_line_with_parent(uuid, None, text)
}

/// Build a JSONL line for an `assistant` entry, optionally with a usage block
/// so the loader's token aggregation has something to sum.
fn assistant_line(
    uuid: &str,
    parent: &str,
    text: &str,
    model: &str,
    usage: Option<(i64, i64)>,
) -> String {
    let mut entry = json!({
        "type": "assistant",
        "uuid": uuid,
        "parent_uuid": parent,
        "session_id": "s1",
        "cwd": "/tmp/p",
        "timestamp": "2025-01-15T10:00:01Z",
        "is_sidechain": false,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
        },
        "model": model,
    });
    if let Some((input, output)) = usage {
        entry["usage"] = json!({
            "input_tokens": input,
            "output_tokens": output,
        });
    }
    serde_json::to_string(&entry).unwrap()
}

fn compact_boundary_line(
    uuid: &uuid::Uuid,
    preserved_segment: Option<serde_json::Value>,
) -> String {
    let mut message = json!({
        "kind": "compact_boundary",
        "uuid": uuid,
        "tokens_before": 50_000,
        "tokens_after": 8_000,
        "trigger": "auto",
    });
    if let Some(segment) = preserved_segment {
        message["preserved_segment"] = segment;
    }
    serde_json::to_string(&json!({
        "type": "system",
        "uuid": uuid,
        "session_id": "s1",
        "cwd": "/tmp/p",
        "timestamp": "2025-01-15T10:00:03Z",
        "is_sidechain": false,
        "message": message,
    }))
    .unwrap()
}

fn preserved_segment_json(
    head_uuid: &uuid::Uuid,
    anchor_uuid: &uuid::Uuid,
    tail_uuid: &uuid::Uuid,
) -> serde_json::Value {
    json!({
        "head_uuid": head_uuid,
        "anchor_uuid": anchor_uuid,
        "tail_uuid": tail_uuid,
    })
}

fn selected_contains(state: &SessionResumeState, uuid: &uuid::Uuid) -> bool {
    state
        .selected_chain_uuids
        .contains(uuid.to_string().as_str())
}

#[test]
fn test_load_conversation_for_resume_basic_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.jsonl");
    // DAG: user → assistant → user → assistant.
    // u2's parent is a1 so the leaf walker reconstructs the full
    // chain rather than stopping at the disconnected sub-tree.
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        user_line("u1", "first prompt"),
        assistant_line(
            "a1",
            "u1",
            "first reply",
            "claude-sonnet-4-6",
            Some((10, 20))
        ),
        user_line_with_parent("u2", Some("a1"), "second prompt"),
        assistant_line(
            "a2",
            "u2",
            "second reply",
            "claude-sonnet-4-6",
            Some((30, 40))
        ),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");

    // 4 messages total — alternating user/assistant.
    assert_eq!(conversation.messages.len(), 4);
    assert!(matches!(conversation.messages[0], Message::User(_)));
    assert!(matches!(conversation.messages[1], Message::Assistant(_)));
    assert!(matches!(conversation.messages[2], Message::User(_)));
    assert!(matches!(conversation.messages[3], Message::Assistant(_)));

    // turn_count counts assistant entries.
    assert_eq!(conversation.turn_count, 2);

    // Latest model wins (both are the same here).
    assert_eq!(conversation.model, "claude-sonnet-4-6");

    // Token aggregation across the two assistant turns.
    assert_eq!(conversation.total_input_tokens, 40);
    assert_eq!(conversation.total_output_tokens, 60);

    // No sidechain in this transcript.
    assert!(!conversation.has_sidechain);
    assert!(conversation.plan_slug.is_none());
}

#[test]
fn test_load_conversation_for_resume_skips_metadata_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s2.jsonl");
    // Metadata-typed entries that should be skipped during recovery.
    let metadata_lines: Vec<String> = [
        "custom-title",
        "tag",
        "last-prompt",
        "summary",
        "cost-summary",
    ]
    .iter()
    .map(|t| {
        serde_json::to_string(&json!({
            "type": t,
            "session_id": "s2",
        }))
        .unwrap()
    })
    .collect();
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        metadata_lines.join("\n"),
        user_line("u1", "the only real prompt"),
        assistant_line("a1", "u1", "reply", "claude-sonnet-4-6", None),
        // Trailing blank + metadata to confirm robustness.
        "",
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");

    // Only the user + assistant entries materialize as messages.
    assert_eq!(
        conversation.messages.len(),
        2,
        "metadata + blank lines should be skipped",
    );
    assert_eq!(conversation.turn_count, 1);
}

#[test]
fn test_load_conversation_for_resume_sidechain_flag_and_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s3.jsonl");
    // One real exchange plus a sidechain entry that must be excluded
    // from messages but flip `has_sidechain`.
    let sidechain = serde_json::to_string(&json!({
        "type": "user",
        "uuid": "side1",
        "session_id": "s3",
        "is_sidechain": true,
        "message": {"role": "user", "content": "subagent prompt"},
    }))
    .unwrap();
    let body = format!(
        "{}\n{}\n{}\n",
        user_line("u1", "main prompt"),
        sidechain,
        assistant_line("a1", "u1", "main reply", "claude-sonnet-4-6", None),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");
    assert!(conversation.has_sidechain, "sidechain flag should flip");
    assert_eq!(
        conversation.messages.len(),
        2,
        "sidechain entry must not appear in main messages: got {} msgs",
        conversation.messages.len(),
    );
}

#[test]
fn test_load_conversation_for_resume_plan_slug_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s4.jsonl");
    // Embed a slug field on a transcript entry — recovery extracts the
    // first non-empty slug it sees.
    let with_slug = serde_json::to_string(&json!({
        "type": "user",
        "uuid": "u1",
        "session_id": "s4",
        "is_sidechain": false,
        "slug": "rebuild-cache",
        "message": {"role": "user", "content": "kick off plan"},
    }))
    .unwrap();
    let body = format!(
        "{}\n{}\n",
        with_slug,
        assistant_line("a1", "u1", "ack", "claude-sonnet-4-6", None),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");
    assert_eq!(conversation.plan_slug.as_deref(), Some("rebuild-cache"));
}

#[test]
fn test_load_conversation_for_resume_missing_file_errors() {
    let err = load_conversation_for_resume(Path::new("/no/such/transcript.jsonl"))
        .expect_err("missing transcript should error");
    let msg = err.to_string();
    assert!(
        msg.contains("transcript not found"),
        "unexpected error message: {msg}",
    );
}

#[test]
fn test_load_conversation_for_resume_invalid_lines_skipped() {
    // Malformed JSONL lines must not abort the whole resume — recovery
    // is a best-effort path used after a crash.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s5.jsonl");
    let body = format!(
        "{}\nthis is not json at all\n{{\"type\":\"user\",\"uuid\":\"\",\n{}\n",
        user_line("u1", "good prompt"),
        assistant_line("a1", "u1", "good reply", "claude-sonnet-4-6", Some((5, 5))),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume tolerates bad lines");
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.turn_count, 1);
    assert_eq!(conversation.total_input_tokens, 5);
    assert_eq!(conversation.total_output_tokens, 5);
}

#[test]
fn test_load_conversation_for_resume_latest_model_wins() {
    // Two assistant turns from different models — conversation.model is
    // the latest non-empty one (per recovery.rs "latest wins"). DAG
    // links u2 → a1 so the leaf walker reconstructs both turns.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s6.jsonl");
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        user_line("u1", "p1"),
        assistant_line("a1", "u1", "r1", "claude-sonnet-4-6", None),
        user_line_with_parent("u2", Some("a1"), "p2"),
        assistant_line("a2", "u2", "r2", "claude-opus-4-7", None),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");
    assert_eq!(conversation.model, "claude-opus-4-7");
    assert_eq!(conversation.turn_count, 2);
}

#[test]
fn test_load_session_state_for_resume_relinks_live_compact_preserved_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compact-live.jsonl");
    let old_user = uuid::Uuid::new_v4();
    let old_assistant = uuid::Uuid::new_v4();
    let head = uuid::Uuid::new_v4();
    let tail = uuid::Uuid::new_v4();
    let boundary = uuid::Uuid::new_v4();
    let summary = uuid::Uuid::new_v4();
    let future_user = uuid::Uuid::new_v4();
    let body = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        user_line(old_user.to_string().as_str(), "old prompt"),
        assistant_line(
            old_assistant.to_string().as_str(),
            old_user.to_string().as_str(),
            "old reply",
            "claude-sonnet-4-6",
            Some((100, 10))
        ),
        user_line_with_parent(
            head.to_string().as_str(),
            Some(old_assistant.to_string().as_str()),
            "kept prompt"
        ),
        assistant_line(
            tail.to_string().as_str(),
            head.to_string().as_str(),
            "kept reply",
            "claude-sonnet-4-6",
            Some((190_000, 200))
        ),
        compact_boundary_line(
            &boundary,
            Some(preserved_segment_json(&head, &summary, &tail))
        ),
        assistant_line(
            summary.to_string().as_str(),
            boundary.to_string().as_str(),
            "compact summary",
            "claude-sonnet-4-6",
            Some((7, 8))
        ),
        user_line_with_parent(
            future_user.to_string().as_str(),
            Some(summary.to_string().as_str()),
            "after compact"
        ),
    );
    std::fs::write(&path, body).unwrap();

    let state = load_session_state_for_resume(&path).expect("resume loads");

    assert!(selected_contains(&state, &boundary));
    assert!(selected_contains(&state, &summary));
    assert!(selected_contains(&state, &head));
    assert!(selected_contains(&state, &tail));
    assert!(selected_contains(&state, &future_user));
    assert!(!selected_contains(&state, &old_user));
    assert!(!selected_contains(&state, &old_assistant));
    assert_eq!(
        state.total_input_tokens, 7,
        "preserved assistant usage must be zeroed on resume"
    );
    assert_eq!(state.total_output_tokens, 8);
}

#[test]
fn test_load_session_state_for_resume_prunes_stale_compact_preserved_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compact-stale.jsonl");
    let head = uuid::Uuid::new_v4();
    let tail = uuid::Uuid::new_v4();
    let first_boundary = uuid::Uuid::new_v4();
    let summary = uuid::Uuid::new_v4();
    let second_boundary = uuid::Uuid::new_v4();
    let final_assistant = uuid::Uuid::new_v4();
    let body = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n",
        user_line(head.to_string().as_str(), "kept prompt"),
        assistant_line(
            tail.to_string().as_str(),
            head.to_string().as_str(),
            "kept reply",
            "claude-sonnet-4-6",
            Some((100, 10))
        ),
        compact_boundary_line(
            &first_boundary,
            Some(preserved_segment_json(&head, &summary, &tail))
        ),
        assistant_line(
            summary.to_string().as_str(),
            first_boundary.to_string().as_str(),
            "first summary",
            "claude-sonnet-4-6",
            Some((11, 12))
        ),
        compact_boundary_line(&second_boundary, None),
        assistant_line(
            final_assistant.to_string().as_str(),
            second_boundary.to_string().as_str(),
            "second summary",
            "claude-sonnet-4-6",
            Some((3, 4))
        ),
    );
    std::fs::write(&path, body).unwrap();

    let state = load_session_state_for_resume(&path).expect("resume loads");

    assert!(selected_contains(&state, &second_boundary));
    assert!(selected_contains(&state, &final_assistant));
    assert!(!selected_contains(&state, &first_boundary));
    assert!(!selected_contains(&state, &summary));
    assert!(!selected_contains(&state, &head));
    assert!(!selected_contains(&state, &tail));
    assert_eq!(state.total_input_tokens, 3);
    assert_eq!(state.total_output_tokens, 4);
}

/// tool_use / tool_result blocks must round-trip on resume so
/// the resumed model sees its own prior tool calls.
#[test]
fn test_load_conversation_for_resume_preserves_tool_blocks() {
    use coco_messages::AssistantContent;
    use coco_messages::LlmMessage;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s7.jsonl");

    // Assistant message with a text part + tool_call block. Stored
    // shape mirrors what `engine_finalize_turn::record_transcript_tail`
    // writes — `serde_json::to_value(&assistant.message)` on an
    // `LlmMessage::Assistant`.
    let assistant_msg_with_tool = json!({
        "type": "assistant",
        "uuid": "a1",
        "parent_uuid": "u1",
        "session_id": "s7",
        "is_sidechain": false,
        "timestamp": "2025-01-15T10:00:01Z",
        "model": "claude-sonnet-4-6",
        "message": {
            "role": "assistant",
            "content": [
                {"type": "text", "text": "let me check"},
                {
                    "type": "tool-call",
                    "toolCallId": "call_1",
                    "toolName": "Read",
                    "input": {"file_path": "/tmp/foo.txt"}
                }
            ]
        },
    });
    let body = format!(
        "{}\n{}\n",
        user_line("u1", "read foo"),
        serde_json::to_string(&assistant_msg_with_tool).unwrap(),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");
    assert_eq!(conversation.messages.len(), 2);

    // Assistant message must round-trip both the text and the
    // tool_call so resumed turns can match the tool_result against
    // a real prior tool_use.
    let Message::Assistant(assistant) = &conversation.messages[1] else {
        panic!("expected assistant message at index 1");
    };
    let LlmMessage::Assistant { content, .. } = &assistant.message else {
        panic!("expected assistant LlmMessage variant");
    };
    let has_tool_call = content
        .iter()
        .any(|c| matches!(c, AssistantContent::ToolCall(_)));
    assert!(
        has_tool_call,
        "tool_call must round-trip; got content: {content:?}"
    );
}

#[test]
fn test_load_conversation_for_resume_tool_result_user_block() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s8.jsonl");
    let assistant = json!({
        "type": "assistant",
        "uuid": "a1",
        "parent_uuid": "u1",
        "session_id": "s8",
        "is_sidechain": false,
        "timestamp": "2025-01-15T10:00:01Z",
        "model": "claude-sonnet-4-6",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool-call",
                "toolCallId": "toolu_1",
                "toolName": "Read",
                "input": {"file_path": "a.txt"}
            }],
        },
    });
    let tool_result = json!({
        "type": "user",
        "uuid": "tr1",
        "parent_uuid": "a1",
        "session_id": "s8",
        "is_sidechain": false,
        "timestamp": "2025-01-15T10:00:02Z",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "tool_name": "Read",
                "content": "file contents"
            }],
        },
    });
    let body = format!(
        "{}\n{}\n{}\n",
        user_line("u1", "read"),
        serde_json::to_string(&assistant).unwrap(),
        serde_json::to_string(&tool_result).unwrap(),
    );
    std::fs::write(&path, body).unwrap();

    let conversation = load_conversation_for_resume(&path).expect("resume loads");
    assert_eq!(conversation.messages.len(), 3);
    let Message::ToolResult(result) = &conversation.messages[2] else {
        panic!("expected tool result, got {:?}", conversation.messages[2]);
    };
    assert_eq!(result.tool_use_id, "toolu_1");
    assert_eq!(result.tool_id.to_string(), "Read");
}

#[test]
fn test_load_session_state_for_resume_splits_multi_tool_result_blocks_with_unique_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s9.jsonl");
    let assistant_uuid = uuid::Uuid::new_v4();
    let tool_result = json!({
        "type": "user",
        "uuid": "11111111-1111-4111-8111-111111111111",
        "parent_uuid": assistant_uuid.to_string(),
        "session_id": "s9",
        "is_sidechain": false,
        "timestamp": "2025-01-15T10:00:02Z",
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "tool_name": "Read",
                    "content": "one"
                },
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_2",
                    "tool_name": "Read",
                    "content": "two"
                }
            ]
        },
    });
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&tool_result).unwrap()),
    )
    .unwrap();

    let resume_state = load_session_state_for_resume(&path).expect("resume loads");
    assert_eq!(resume_state.messages.len(), 2);
    let ids = resume_state
        .messages
        .iter()
        .filter_map(Message::uuid)
        .copied()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 2, "split tool results must not share UUIDs");
    for message in &resume_state.messages {
        let Message::ToolResult(result) = message else {
            panic!("expected tool result, got {message:?}");
        };
        assert_eq!(result.source_assistant_uuid, Some(assistant_uuid));
    }
}
