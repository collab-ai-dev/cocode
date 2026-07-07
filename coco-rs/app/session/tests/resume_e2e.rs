//! Resume pipeline end-to-end tests.
//!
//! These exercise `coco_session::recovery::load_session_state_for_resume`
//! against synthetic JSONL transcripts that include the full range of
//! entry types coco-rs writes: plain user/assistant turns, compact and
//! microcompact boundaries, attachments, content-replacement records
//! for budgeted tool results (including big payloads), file-history
//! snapshots (initial + updates), marble-origami staged commits, and
//! subagent-scoped records.
//!
//! Each test:
//!   1. Builds a JSONL file (raw lines or via `TranscriptStore`).
//!   2. Calls the resume entry point.
//!   3. Asserts the reconstructed messages + auxiliary state match.
//!
//! The wire format is **snake_case** per the policy documented in
//! `coco-session/CLAUDE.md` — fixtures use snake_case field names
//! directly so they reflect what the storage layer actually writes.

// Integration-test file: helpers below are not `#[test]`-annotated so
// clippy's `allow-unwrap-in-tests` / `allow-expect-in-tests` shields
// don't reach them. Permitting `.unwrap()` / `.expect(...)` at file
// scope keeps the fixture builders readable without scattering
// `.map_err(|_| ...)` plumbing throughout.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use coco_messages::Message;
use coco_paths::ProjectPaths;
use coco_session::ContentReplacementRecord;
use coco_session::MetadataEntry;
use coco_session::TranscriptStore;
use coco_session::recovery::load_session_state_for_resume;
use coco_session::storage::ChainWriteOptions;
use coco_types::AttachmentKind;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

const SESSION: &str = "s-e2e";
const CWD: &str = "/tmp/coco-e2e";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_session_id(value: &str) -> coco_types::SessionId {
    coco_types::SessionId::try_new(value).unwrap()
}

fn fresh_store() -> (TempDir, TranscriptStore, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let paths = Arc::new(ProjectPaths::new(
        dir.path().to_path_buf(),
        std::path::Path::new(CWD),
    ));
    let path = paths.transcript(SESSION);
    let store = TranscriptStore::new(paths);
    (dir, store, path)
}

fn write_chain(store: &TranscriptStore, msgs: &[Message]) {
    let mut seen: HashSet<Uuid> = HashSet::new();
    let opts = ChainWriteOptions {
        cwd: CWD.to_string(),
        timestamp: "2025-01-15T10:00:00Z".to_string(),
        is_sidechain: false,
        agent_id: None,
        starting_parent_uuid: None,
        git_branch: Some("main".to_string()),
    };
    store
        .append_message_chain(SESSION, msgs.iter(), &mut seen, opts)
        .expect("chain write");
}

fn user_line(uuid: &str, parent: Option<&str>, text: &str, ts: &str) -> String {
    let mut e = json!({
        "type": "user",
        "uuid": uuid,
        "session_id": SESSION,
        "cwd": CWD,
        "timestamp": ts,
        "is_sidechain": false,
        "message": {"role": "user", "content": [{"type": "text", "text": text}]},
    });
    if let Some(p) = parent {
        e["parent_uuid"] = json!(p);
    }
    serde_json::to_string(&e).unwrap()
}

fn assistant_line(uuid: &str, parent: &str, text: &str, ts: &str) -> String {
    let e = json!({
        "type": "assistant",
        "uuid": uuid,
        "parent_uuid": parent,
        "session_id": SESSION,
        "cwd": CWD,
        "timestamp": ts,
        "is_sidechain": false,
        "model": "claude-sonnet-4-6",
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
        "usage": {"input_tokens": 10, "output_tokens": 5},
    });
    serde_json::to_string(&e).unwrap()
}

fn system_compact_boundary_line(uuid: &str, parent: Option<&str>, ts: &str) -> String {
    // On disk, the engine writes SystemMessage with `tag = "kind"`. The
    // outer entry's `parent_uuid` is null when this is treated as a
    // chain break, but we only assert chain-walk *behavior*
    // — parent linkage is what really matters here. Compact boundary
    // does NOT truncate stored entries per the recovery fix.
    let mut e = json!({
        "type": "system",
        "uuid": uuid,
        "session_id": SESSION,
        "cwd": CWD,
        "timestamp": ts,
        "is_sidechain": false,
        "message": {
            "kind": "compact_boundary",
            "uuid": uuid,
            "tokens_before": 50_000,
            "tokens_after": 8_000,
            "trigger": "auto",
        },
    });
    if let Some(p) = parent {
        e["parent_uuid"] = json!(p);
    }
    serde_json::to_string(&e).unwrap()
}

fn system_microcompact_boundary_line(uuid: &str, parent: &str, ts: &str) -> String {
    let e = json!({
        "type": "system",
        "uuid": uuid,
        "parent_uuid": parent,
        "session_id": SESSION,
        "cwd": CWD,
        "timestamp": ts,
        "is_sidechain": false,
        "message": {
            "kind": "microcompact_boundary",
            "uuid": uuid,
        },
    });
    serde_json::to_string(&e).unwrap()
}

fn write_lines(path: &std::path::Path, lines: &[String]) {
    let body = lines.join("\n") + "\n";
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn assistant_with_uuid(uuid: Uuid, text: &str) -> Message {
    use coco_messages::AssistantContent;
    use coco_messages::AssistantMessage;
    use coco_messages::LlmMessage;
    use coco_messages::TextContent;
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![AssistantContent::Text(TextContent {
                text: text.to_string(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid,
        model: "claude-sonnet-4-6".to_string(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn assistant_tool_call(tool_use_id: &str, tool_name: &str) -> Message {
    coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::ToolCall(
            coco_messages::ToolCallContent::new(
                tool_use_id.to_string(),
                tool_name.to_string(),
                json!({"query": tool_use_id}),
            ),
        )],
        "claude-sonnet-4-6",
        coco_types::TokenUsage::default(),
    )
}

fn tool_result(tool_use_id: &str, tool_name: &str, output: &str) -> Message {
    coco_messages::create_tool_result_message(
        tool_use_id,
        tool_name,
        coco_types::ToolId::Custom(tool_name.to_string()),
        output,
        /*is_error*/ false,
    )
}

fn write_agent_chain(store: &TranscriptStore, agent_id: &str, msgs: &[Message]) {
    let mut seen: HashSet<Uuid> = HashSet::new();
    store
        .append_agent_message_chain(
            SESSION,
            agent_id,
            msgs.iter(),
            &mut seen,
            ChainWriteOptions {
                cwd: CWD.to_string(),
                timestamp: "2025-01-15T10:00:00Z".to_string(),
                is_sidechain: true,
                agent_id: Some(agent_id.to_string()),
                starting_parent_uuid: None,
                git_branch: Some("main".to_string()),
            },
        )
        .expect("agent chain write");
}

fn budget_replacement(tool_use_id: &str, output: &str) -> String {
    format!(
        "<persisted-output>\nOutput too large ({} bytes). Full output saved to: /tmp/{tool_use_id}.txt\n\nPreview (first 2KB):\n{}\n...\n</persisted-output>",
        output.len(),
        &output[..output.floor_char_boundary(128.min(output.len()))],
    )
}

fn replacement_by_id(
    records: &[ContentReplacementRecord],
) -> std::collections::HashMap<&str, &str> {
    records
        .iter()
        .map(|record| (record.tool_use_id(), record.replacement()))
        .collect()
}

fn tool_result_lengths(messages: &[Message]) -> std::collections::HashMap<String, usize> {
    let mut out = std::collections::HashMap::new();
    for message in messages {
        let Message::ToolResult(tool_result) = message else {
            continue;
        };
        let coco_messages::LlmMessage::Tool { content, .. } = &tool_result.message else {
            continue;
        };
        for part in content {
            let coco_messages::ToolContent::ToolResult(part) = part else {
                continue;
            };
            if let coco_messages::ToolResultOutput::Text { value, .. } = &part.output {
                out.insert(tool_result.tool_use_id.clone(), value.len());
            }
        }
    }
    out
}

fn user_with_uuid(uuid: Uuid, text: &str) -> Message {
    coco_messages::create_user_message_with_uuid(uuid, text)
}

// ---------------------------------------------------------------------------
// Compact / microcompact boundary
// ---------------------------------------------------------------------------

/// `compact_boundary` does NOT truncate prior messages from the chain.
/// The recovery fix removed an `entries.drain(0..idx)` that used to
/// throw away the pre-compact prefix; the reconstructed chain must
/// still surface every linked turn regardless of how many boundaries
/// preceded it.
#[test]
fn compact_boundary_preserves_pre_compact_messages_in_chain() {
    let (_dir, _store, path) = fresh_store();
    let lines = vec![
        user_line("u1", None, "pre-compact prompt", "2025-01-15T10:00:00Z"),
        assistant_line("a1", "u1", "pre-compact reply", "2025-01-15T10:00:01Z"),
        system_compact_boundary_line("cb", Some("a1"), "2025-01-15T10:00:02Z"),
        user_line(
            "u2",
            Some("cb"),
            "post-compact prompt",
            "2025-01-15T10:00:03Z",
        ),
        assistant_line("a2", "u2", "post-compact reply", "2025-01-15T10:00:04Z"),
    ];
    write_lines(&path, &lines);

    let state = load_session_state_for_resume(&path).expect("load");
    // All four conversation turns must be present — the boundary is a
    // system message that lives between them, but recovery walks back
    // from the leaf `a2` and reaches every ancestor.
    let user_count = state
        .messages
        .iter()
        .filter(|m| matches!(m, Message::User(_)))
        .count();
    let asst_count = state
        .messages
        .iter()
        .filter(|m| matches!(m, Message::Assistant(_)))
        .count();
    assert_eq!(user_count, 2, "both user turns survive across the boundary");
    assert_eq!(
        asst_count, 2,
        "both assistant turns survive across the boundary",
    );
}

/// `microcompact_boundary` is a plain system message; it MUST NOT be
/// treated as a chain-breaking boundary. The recovery fix dropped the
/// over-broad `is_compact_boundary_entry` predicate that conflated the
/// two subtypes.
#[test]
fn microcompact_boundary_is_inline_not_chain_break() {
    let (_dir, _store, path) = fresh_store();
    let lines = vec![
        user_line("u1", None, "first", "2025-01-15T10:00:00Z"),
        assistant_line("a1", "u1", "reply 1", "2025-01-15T10:00:01Z"),
        system_microcompact_boundary_line("mc", "a1", "2025-01-15T10:00:02Z"),
        user_line("u2", Some("mc"), "second", "2025-01-15T10:00:03Z"),
        assistant_line("a2", "u2", "reply 2", "2025-01-15T10:00:04Z"),
    ];
    write_lines(&path, &lines);

    let state = load_session_state_for_resume(&path).expect("load");
    // Both turns must be in the chain. Earlier code conflated
    // microcompact with compact_boundary and silently dropped every
    // pre-microcompact entry.
    let user_count = state
        .messages
        .iter()
        .filter(|m| matches!(m, Message::User(_)))
        .count();
    assert_eq!(
        user_count,
        2,
        "microcompact must not truncate prior turns (got {} messages: {:?})",
        state.messages.len(),
        state
            .messages
            .iter()
            .map(|m| match m {
                Message::User(_) => "user",
                Message::Assistant(_) => "assistant",
                Message::System(_) => "system",
                Message::Attachment(_) => "attachment",
                Message::ToolResult(_) => "tool_result",
                Message::Progress(_) => "progress",
                Message::Tombstone(_) => "tombstone",
            })
            .collect::<Vec<_>>(),
    );
}

// ---------------------------------------------------------------------------
// Leaf selection
// ---------------------------------------------------------------------------

/// Two terminal user/assistant leaves with the **same** timestamp
/// must pick the disk-first one (first-wins on equal timestamps via
/// strict `>`).
#[test]
fn multi_leaf_tie_break_picks_first_disk_occurrence() {
    let (_dir, _store, path) = fresh_store();
    let ts = "2025-01-15T10:00:01Z";
    let lines = vec![
        user_line("u1", None, "root", "2025-01-15T10:00:00Z"),
        // Two assistants forking from u1 with identical timestamps —
        // both are leaves (no entry's parent_uuid points at either).
        assistant_line("a-first", "u1", "branch-first", ts),
        assistant_line("a-second", "u1", "branch-second", ts),
    ];
    write_lines(&path, &lines);

    let state = load_session_state_for_resume(&path).expect("load");
    // Walk back from the chosen leaf to confirm which branch survived.
    let asst_text = state.messages.iter().find_map(|m| match m {
        Message::Assistant(a) => match &a.message {
            coco_messages::LlmMessage::Assistant { content, .. } => {
                content.iter().find_map(|c| match c {
                    coco_messages::AssistantContent::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        asst_text.as_deref(),
        Some("branch-first"),
        "tie-break must pick first-wins on equal timestamps (strict `>`)",
    );
}

/// A terminal `attachment` entry must not be picked as the leaf
/// anchor. The walker steps back to the nearest user/assistant
/// ancestor.
#[test]
fn terminal_attachment_falls_back_to_user_assistant_ancestor() {
    let (_dir, _store, path) = fresh_store();
    // u1 ← a1 ← attachment leaf. The attachment is the disk-last entry
    // but it must NOT anchor the walk — we expect to surface a1's
    // reply text in the resumed chain.
    let attachment = json!({
        "type": "attachment",
        "uuid": "att",
        "parent_uuid": "a1",
        "session_id": SESSION,
        "cwd": CWD,
        "timestamp": "2025-01-15T10:00:02Z",
        "is_sidechain": false,
        // Mirror the engine's wire shape: serialized AttachmentMessage
        // value (uuid + kind + body).
        "message": {
            "uuid": "att",
            "kind": "critical_system_reminder",
            "body": {
                "body": "api",
                "role": "user",
                "content": [{"type": "text", "text": "<system-reminder>note</system-reminder>"}],
            }
        }
    });
    let lines = vec![
        user_line("u1", None, "prompt", "2025-01-15T10:00:00Z"),
        assistant_line("a1", "u1", "reply", "2025-01-15T10:00:01Z"),
        serde_json::to_string(&attachment).unwrap(),
    ];
    write_lines(&path, &lines);

    let state = load_session_state_for_resume(&path).expect("load");
    assert!(
        state
            .messages
            .iter()
            .any(|m| matches!(m, Message::Assistant(_))),
        "assistant ancestor must surface; attachment must not anchor leaf walk",
    );
}

// ---------------------------------------------------------------------------
// Content replacement (Level 2 tool result budget)
// ---------------------------------------------------------------------------

/// A content-replacement record persisted in main-thread mode
/// (`agent_id: None`) lands on `SessionResumeState.content_replacements`
/// keyed by `tool_use_id` only. Replacement string round-trips intact
/// regardless of size.
#[test]
fn content_replacement_main_thread_routes_to_main_bucket() {
    let (_dir, store, path) = fresh_store();
    // Establish at least one transcript entry so the resume loader has
    // a chain to anchor.
    let u = user_with_uuid(Uuid::new_v4(), "trigger");
    let a = assistant_with_uuid(Uuid::new_v4(), "ack");
    write_chain(&store, &[u, a]);
    store
        .insert_content_replacement(
            SESSION,
            None,
            &[ContentReplacementRecord::tool_result(
                "toolu_main_1",
                "<persisted-output>preview-A</persisted-output>",
            )],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");
    assert_eq!(state.content_replacements.len(), 1);
    assert_eq!(state.content_replacements[0].tool_use_id(), "toolu_main_1");
    assert_eq!(
        state.content_replacements[0].replacement(),
        "<persisted-output>preview-A</persisted-output>",
    );
    assert!(
        state.agent_content_replacements.is_empty(),
        "no subagent records → agent bucket stays empty",
    );
}

/// Subagent records (written with `agent_id: Some`) land on
/// `agent_content_replacements[agent_id]`, NEVER the main bucket.
/// Multiple agents produce separate buckets.
#[test]
fn content_replacement_subagent_routes_by_agent_id() {
    let (_dir, store, path) = fresh_store();
    let u = user_with_uuid(Uuid::new_v4(), "spawn-agents");
    let a = assistant_with_uuid(Uuid::new_v4(), "spawned");
    write_chain(&store, &[u, a]);

    store
        .insert_content_replacement(
            SESSION,
            Some("agent-a"),
            &[ContentReplacementRecord::tool_result(
                "toolu_agent_a_1",
                "agent-a replacement",
            )],
        )
        .unwrap();
    store
        .insert_content_replacement(
            SESSION,
            Some("agent-b"),
            &[ContentReplacementRecord::tool_result(
                "toolu_agent_b_1",
                "agent-b replacement",
            )],
        )
        .unwrap();
    // Main-thread record co-exists; must NOT pollute the agent buckets.
    store
        .insert_content_replacement(
            SESSION,
            None,
            &[ContentReplacementRecord::tool_result(
                "toolu_main_2",
                "main replacement",
            )],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");

    assert_eq!(state.content_replacements.len(), 1);
    assert_eq!(state.content_replacements[0].tool_use_id(), "toolu_main_2");

    let agent_a = state
        .agent_content_replacements
        .get("agent-a")
        .expect("agent-a bucket present");
    assert_eq!(agent_a.len(), 1);
    assert_eq!(agent_a[0].tool_use_id(), "toolu_agent_a_1");

    let agent_b = state
        .agent_content_replacements
        .get("agent-b")
        .expect("agent-b bucket present");
    assert_eq!(agent_b.len(), 1);
    assert_eq!(agent_b[0].tool_use_id(), "toolu_agent_b_1");

    // Physical routing (mirror TS): the agent records live in their per-agent
    // transcripts, NEVER the main file — which carries only the main record.
    let main_content = std::fs::read_to_string(&path).unwrap();
    assert!(main_content.contains("toolu_main_2"));
    assert!(
        !main_content.contains("toolu_agent_a_1") && !main_content.contains("toolu_agent_b_1"),
        "subagent content-replacement must not land in the main transcript",
    );
    let agent_a_path = store.agent_transcript_path(SESSION, "agent-a");
    assert!(
        agent_a_path.exists(),
        "per-agent content-replacement file must exist"
    );
    assert!(
        std::fs::read_to_string(&agent_a_path)
            .unwrap()
            .contains("toolu_agent_a_1"),
    );
}

/// Multi-megabyte replacement strings survive write + reload intact —
/// guards against accidental size caps or content truncation
/// somewhere in the persistence path.
#[test]
fn big_tool_result_replacement_survives_resume() {
    let (_dir, store, path) = fresh_store();
    let u = user_with_uuid(Uuid::new_v4(), "big tool call");
    let a = assistant_with_uuid(Uuid::new_v4(), "ran the big tool");
    write_chain(&store, &[u, a]);

    // 256 KB payload — larger than the head/tail lite-read window so
    // the load path has to take the full-read branch.
    let big = "X".repeat(256 * 1024);
    store
        .insert_content_replacement(
            SESSION,
            None,
            &[ContentReplacementRecord::tool_result("toolu_big", &big)],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");
    assert_eq!(state.content_replacements.len(), 1);
    assert_eq!(state.content_replacements[0].replacement().len(), big.len());
    assert_eq!(state.content_replacements[0].replacement(), big);
}

/// Full regression for resume after Level 2 tool-result budget fired in both
/// the main thread and a subagent:
///
/// - main transcript keeps the original big `tool_result` message;
/// - subagent transcript lives in `<sid>/subagents/agent-*.jsonl` and also
///   keeps its original big `tool_result`;
/// - budget-generated `content-replacement` records are routed to the correct
///   resume buckets (`content_replacements` vs `agent_content_replacements`);
/// - replacement strings are byte-stable and big enough to prove this was the
///   budget/persisted-output path, not a small inline result.
#[test]
fn resume_restores_main_and_subagent_big_tool_result_budget_state() {
    let (_dir, store, path) = fresh_store();
    let agent_id = "agent-budget-1";
    let main_tool_use_id = "toolu_main_big";
    let agent_tool_use_id = "toolu_agent_big";
    let main_big_output = format!("main-big-start\n{}\nmain-big-end", "M".repeat(220 * 1024));
    let agent_big_output = format!("agent-big-start\n{}\nagent-big-end", "A".repeat(230 * 1024));

    let main_messages = vec![
        user_with_uuid(Uuid::new_v4(), "run the main big tool"),
        assistant_tool_call(main_tool_use_id, "MainBigTool"),
        tool_result(main_tool_use_id, "MainBigTool", &main_big_output),
        assistant_with_uuid(Uuid::new_v4(), "main tool summarized"),
    ];
    write_chain(&store, &main_messages);

    let agent_messages = vec![
        user_with_uuid(Uuid::new_v4(), "run the subagent big tool"),
        assistant_tool_call(agent_tool_use_id, "AgentBigTool"),
        tool_result(agent_tool_use_id, "AgentBigTool", &agent_big_output),
        assistant_with_uuid(Uuid::new_v4(), "agent tool summarized"),
    ];
    write_agent_chain(&store, agent_id, &agent_messages);

    let main_replacement = budget_replacement(main_tool_use_id, &main_big_output);
    let agent_replacement = budget_replacement(agent_tool_use_id, &agent_big_output);
    store
        .insert_content_replacement(
            SESSION,
            None,
            &[ContentReplacementRecord::tool_result(
                main_tool_use_id,
                &main_replacement,
            )],
        )
        .unwrap();
    store
        .insert_content_replacement(
            SESSION,
            Some(agent_id),
            &[ContentReplacementRecord::tool_result(
                agent_tool_use_id,
                &agent_replacement,
            )],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("main resume state loads");
    let main_tool_results = tool_result_lengths(&state.messages);
    assert_eq!(
        main_tool_results.get(main_tool_use_id),
        Some(&main_big_output.len()),
        "main resume must keep the original big tool_result in transcript history",
    );

    let main_records = replacement_by_id(&state.content_replacements);
    assert_eq!(
        main_records.get(main_tool_use_id).copied(),
        Some(main_replacement.as_str()),
        "main budget replacement must seed the main resume bucket",
    );
    assert!(
        !main_records.contains_key(agent_tool_use_id),
        "agent replacement must not leak into main bucket",
    );

    let agent_records = state
        .agent_content_replacements
        .get(agent_id)
        .expect("agent budget bucket present");
    let agent_records = replacement_by_id(agent_records);
    assert_eq!(
        agent_records.get(agent_tool_use_id).copied(),
        Some(agent_replacement.as_str()),
        "agent budget replacement must seed the agent-specific resume bucket",
    );
    assert!(
        !agent_records.contains_key(main_tool_use_id),
        "main replacement must not leak into agent bucket",
    );

    let agent_loaded_messages = store
        .load_agent_messages(SESSION, agent_id)
        .unwrap()
        .expect("agent transcript present");
    let agent_loaded_messages: Vec<Message> = agent_loaded_messages
        .iter()
        .map(|message| message.as_ref().clone())
        .collect();
    let agent_tool_results = tool_result_lengths(&agent_loaded_messages);
    assert_eq!(
        agent_tool_results.get(agent_tool_use_id),
        Some(&agent_big_output.len()),
        "subagent resume must keep the original big tool_result in its own transcript",
    );

    let main_content = std::fs::read_to_string(&path).unwrap();
    assert!(main_content.contains(main_tool_use_id));
    assert!(!main_content.contains(agent_tool_use_id));
    let agent_content = std::fs::read_to_string(store.agent_transcript_path(SESSION, agent_id))
        .expect("agent transcript file readable");
    assert!(agent_content.contains(agent_tool_use_id));
    assert!(!agent_content.contains(main_tool_use_id));
    assert!(main_replacement.contains("<persisted-output>"));
    assert!(agent_replacement.contains("<persisted-output>"));
}

// ---------------------------------------------------------------------------
// File-history snapshot chain
// ---------------------------------------------------------------------------

/// The chain is walked in **conversation** order — snapshots tied to
/// messages outside the resumed chain are skipped, and snapshots inside
/// it appear in chain-order, not disk-append order.
#[test]
fn file_history_snapshot_chain_walks_conversation_order() {
    let (_dir, store, path) = fresh_store();
    let u1 = Uuid::new_v4();
    let a1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    let a2 = Uuid::new_v4();
    write_chain(
        &store,
        &[
            user_with_uuid(u1, "prompt 1"),
            assistant_with_uuid(a1, "reply 1"),
            user_with_uuid(u2, "prompt 2"),
            assistant_with_uuid(a2, "reply 2"),
        ],
    );

    // Append snapshots in REVERSE conversation order on disk to prove
    // chain-order walk wins over disk-append order.
    store
        .insert_file_history_snapshot(
            SESSION,
            &u2.to_string(),
            json!({"message_id": u2.to_string(), "version": "v2"}),
            false,
        )
        .unwrap();
    store
        .insert_file_history_snapshot(
            SESSION,
            &u1.to_string(),
            json!({"message_id": u1.to_string(), "version": "v1"}),
            false,
        )
        .unwrap();
    // Snapshot for a message NOT in the chain — must be dropped.
    store
        .insert_file_history_snapshot(
            SESSION,
            "unrelated-msg",
            json!({"message_id": "unrelated-msg", "version": "phantom"}),
            false,
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");
    let versions: Vec<&str> = state
        .file_history_snapshots
        .iter()
        .map(|s| s.get("version").and_then(Value::as_str).unwrap_or(""))
        .collect();
    assert_eq!(
        versions,
        vec!["v1", "v2"],
        "chain order wins; phantom snapshot dropped",
    );
}

/// `is_snapshot_update = true` overwrites the slot keyed by the
/// **inner** `snapshot.message_id`, not the outer entry's message_id.
/// The current turn's message_id is the outer field while the original
/// snapshot's inner message_id identifies the slot to update.
#[test]
fn file_history_snapshot_update_overwrites_by_inner_message_id() {
    let (_dir, store, path) = fresh_store();
    let u1 = Uuid::new_v4();
    let a1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    write_chain(
        &store,
        &[
            user_with_uuid(u1, "create file"),
            assistant_with_uuid(a1, "edited"),
            user_with_uuid(u2, "edit again"),
        ],
    );

    // Initial snapshot at u1.
    store
        .insert_file_history_snapshot(
            SESSION,
            &u1.to_string(),
            json!({"message_id": u1.to_string(), "files": {"a.txt": {"v": 1}}}),
            false,
        )
        .unwrap();

    // Update entry: outer message_id = u2 (current turn), inner
    // snapshot.message_id = u1 (the snapshot being overwritten).
    store
        .insert_file_history_snapshot(
            SESSION,
            &u2.to_string(),
            json!({"message_id": u1.to_string(), "files": {"a.txt": {"v": 2}}}),
            true,
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");
    assert_eq!(
        state.file_history_snapshots.len(),
        1,
        "single slot overwritten"
    );
    assert_eq!(
        state.file_history_snapshots[0]
            .pointer("/files/a.txt/v")
            .and_then(Value::as_i64),
        Some(2),
        "in-place update wins"
    );
}

// ---------------------------------------------------------------------------
// Sidechain isolation
// ---------------------------------------------------------------------------

/// A transcript that mixes a real main-thread chain with sidechain
/// (subagent) entries must surface only the main chain on resume.
#[test]
fn sidechain_entries_excluded_from_main_chain() {
    let (_dir, _store, path) = fresh_store();
    // Main chain.
    let main_u = user_line("u1", None, "main prompt", "2025-01-15T10:00:00Z");
    let main_a = assistant_line("a1", "u1", "main reply", "2025-01-15T10:00:01Z");
    // Sidechain — gets filtered out.
    let mut side_u_val = serde_json::from_str::<Value>(&user_line(
        "su1",
        None,
        "sidechain prompt",
        "2025-01-15T10:00:00Z",
    ))
    .unwrap();
    side_u_val["is_sidechain"] = json!(true);
    let side_u = serde_json::to_string(&side_u_val).unwrap();

    write_lines(&path, &[main_u, main_a, side_u]);

    let state = load_session_state_for_resume(&path).expect("load");
    assert!(state.has_sidechain, "sidechain presence is flagged");
    assert!(
        !state.messages.iter().any(|m| match m {
            Message::User(u) => match &u.message {
                coco_messages::LlmMessage::User { content, .. } =>
                    content.iter().any(|c| matches!(
                        c,
                        coco_messages::UserContent::Text(t) if t.text.contains("sidechain prompt"),
                    )),
                _ => false,
            },
            _ => false,
        }),
        "sidechain prompt must NOT appear in the main chain",
    );
}

// ---------------------------------------------------------------------------
// Aggregates
// ---------------------------------------------------------------------------

/// The resume state aggregates per-entry token usage, turn count, and
/// the most-recent assistant model across the resumed chain.
#[test]
fn session_resume_state_aggregates_tokens_and_turn_count() {
    let (_dir, _store, path) = fresh_store();
    let lines = vec![
        user_line("u1", None, "first", "2025-01-15T10:00:00Z"),
        // Two assistant turns with different usage so we can verify
        // the sum.
        {
            let mut e: Value =
                serde_json::from_str(&assistant_line("a1", "u1", "r1", "2025-01-15T10:00:01Z"))
                    .unwrap();
            e["usage"] = json!({"input_tokens": 100, "output_tokens": 25});
            e["model"] = json!("claude-opus-old");
            serde_json::to_string(&e).unwrap()
        },
        user_line("u2", Some("a1"), "second", "2025-01-15T10:00:02Z"),
        {
            let mut e: Value =
                serde_json::from_str(&assistant_line("a2", "u2", "r2", "2025-01-15T10:00:03Z"))
                    .unwrap();
            e["usage"] = json!({"input_tokens": 200, "output_tokens": 50});
            e["model"] = json!("claude-sonnet-new");
            serde_json::to_string(&e).unwrap()
        },
    ];
    write_lines(&path, &lines);

    let state = load_session_state_for_resume(&path).expect("load");
    assert_eq!(state.turn_count, 2);
    assert_eq!(state.total_input_tokens, 300);
    assert_eq!(state.total_output_tokens, 75);
    // Latest assistant model wins (chain walk visits a2 last).
    assert_eq!(state.model, "claude-sonnet-new");
}

// ---------------------------------------------------------------------------
// Marble-origami staged context-collapse
// ---------------------------------------------------------------------------

/// Marble-origami entries are session-scoped via the payload's
/// `session_id` field — entries tagged with a different session must
/// be ignored by the loader.
#[test]
fn marble_origami_entries_filtered_by_session_id() {
    let (_dir, store, _path) = fresh_store();
    let u = user_with_uuid(Uuid::new_v4(), "trigger");
    let a = assistant_with_uuid(Uuid::new_v4(), "ack");
    write_chain(&store, &[u, a]);

    // One commit for this session, one for a stray session id.
    store
        .append_marble_origami_commit(SESSION, json!({"session_id": SESSION, "id": "c1"}))
        .unwrap();
    store
        .append_marble_origami_commit(
            SESSION,
            json!({"session_id": "other-session", "id": "stray"}),
        )
        .unwrap();
    // Snapshot — last-wins for matching session.
    store
        .append_marble_origami_snapshot(SESSION, json!({"session_id": SESSION, "v": "keep"}))
        .unwrap();

    let (commits, snapshot) = store.load_marble_origami_entries(SESSION).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].get("id").and_then(Value::as_str), Some("c1"));
    assert_eq!(
        snapshot
            .as_ref()
            .and_then(|s| s.get("v").and_then(Value::as_str)),
        Some("keep"),
    );
}

// ---------------------------------------------------------------------------
// Attachment round-trip
// ---------------------------------------------------------------------------

/// An `attachment` line written by the engine (full `AttachmentMessage`
/// serialised into the entry's `message` field) is read back as a
/// `Message::Attachment` — not silently dropped. This was the D3 fix.
///
/// To exercise the round-trip we put the attachment between an
/// assistant and a follow-up user message so it sits inside the
/// parent_uuid chain (the leaf walk anchors on user/assistant; a
/// terminal attachment would be skipped).
#[test]
fn attachment_round_trip_preserves_kind_and_body() {
    let (_dir, store, path) = fresh_store();
    let u1 = user_with_uuid(Uuid::new_v4(), "prompt");
    let a1 = assistant_with_uuid(Uuid::new_v4(), "reply");

    // Build an api-visible attachment with a known kind.
    let att = coco_messages::AttachmentMessage::api(
        AttachmentKind::CriticalSystemReminder,
        coco_messages::LlmMessage::user_text("attached reminder body"),
    );
    let att_uuid = att.uuid;
    let att_msg = Message::Attachment(att);

    let u2 = user_with_uuid(Uuid::new_v4(), "follow-up");
    let a2 = assistant_with_uuid(Uuid::new_v4(), "follow-up reply");

    write_chain(&store, &[u1, a1, att_msg, u2, a2]);

    let state = load_session_state_for_resume(&path).expect("load");
    let attachment = state
        .messages
        .iter()
        .find(|m| matches!(m, Message::Attachment(_)));
    assert!(
        attachment.is_some(),
        "attachment in mid-chain must round-trip; chain shape was {:?}",
        state
            .messages
            .iter()
            .map(|m| match m {
                Message::User(_) => "user",
                Message::Assistant(_) => "assistant",
                Message::System(_) => "system",
                Message::Attachment(_) => "attachment",
                Message::ToolResult(_) => "tool_result",
                Message::Progress(_) => "progress",
                Message::Tombstone(_) => "tombstone",
            })
            .collect::<Vec<_>>(),
    );
    if let Some(Message::Attachment(a)) = attachment {
        assert_eq!(a.uuid, att_uuid);
        assert_eq!(a.kind, AttachmentKind::CriticalSystemReminder);
    }
}

// ---------------------------------------------------------------------------
// Metadata side-channels round-trip
// ---------------------------------------------------------------------------

/// Custom-title / tag / last-prompt metadata entries survive the read
/// path even when interleaved with transcript messages — they fall
/// into `read_metadata` rather than the chain walk.
#[test]
fn metadata_side_channels_round_trip() {
    let (_dir, store, _path) = fresh_store();
    let u = user_with_uuid(Uuid::new_v4(), "set up");
    write_chain(&store, &[u]);

    store
        .append_metadata(
            SESSION,
            &MetadataEntry::CustomTitle {
                session_id: test_session_id(SESSION),
                custom_title: "audit run".to_string(),
            },
        )
        .unwrap();
    store
        .append_metadata(
            SESSION,
            &MetadataEntry::Tag {
                session_id: test_session_id(SESSION),
                tag: "bugfix".to_string(),
            },
        )
        .unwrap();
    store
        .append_metadata(
            SESSION,
            &MetadataEntry::LastPrompt {
                session_id: test_session_id(SESSION),
                last_prompt: "final question".to_string(),
                leaf_uuid: None,
                explicit: false,
                rewound: false,
            },
        )
        .unwrap();

    let meta = store.read_metadata(SESSION).expect("read metadata");
    assert_eq!(meta.custom_title.as_deref(), Some("audit run"));
    assert_eq!(meta.tag.as_deref(), Some("bugfix"));
    assert_eq!(meta.last_prompt.as_deref(), Some("final question"));
}

// ---------------------------------------------------------------------------
// Multi-tool-call turn (parallel tool_results in one user message)
// ---------------------------------------------------------------------------

/// A single assistant turn with N parallel `tool_use` calls produces
/// N follow-up `tool_result` content blocks. coco-rs splits those into
/// N separate `Message::ToolResult` in memory but persists them as
/// ONE transcript `user` entry whose `message.content` array has all
/// N blocks. The read side must reconstruct N distinct
/// `Message::ToolResult` again (each with the right `tool_use_id`)
/// and content-replacement records (keyed by `tool_use_id`) must
/// apply across the whole set on resume.
#[test]
fn parallel_tool_results_round_trip_and_replacements_apply() {
    let (_dir, store, path) = fresh_store();
    let u1 = user_with_uuid(Uuid::new_v4(), "list files three ways");
    let a1 = assistant_with_uuid(Uuid::new_v4(), "issuing 3 tool calls");
    write_chain(&store, &[u1, a1]);

    // Synthesize a transcript entry with three tool_result blocks in
    // one user message — this is the shape the wire writer emits when
    // an assistant turn fires N parallel tools that all complete in
    // the same model round.
    let tool_result_user = json!({
        "type": "user",
        "uuid": "tr-aggregate",
        "parent_uuid": "irrelevant", // walker uses source_assistant_uuid mapping below
        "session_id": SESSION,
        "cwd": CWD,
        "timestamp": "2025-01-15T10:00:05Z",
        "is_sidechain": false,
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_a",
                    "tool_name": "Read",
                    "content": "alpha output",
                },
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_b",
                    "tool_name": "Read",
                    "content": "beta output",
                },
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_c",
                    "tool_name": "Read",
                    "content": "gamma output",
                },
            ],
        },
    });
    // Append the aggregate tool_result entry directly so we control
    // the shape exactly.
    let aggregate_line = serde_json::to_string(&tool_result_user).unwrap();
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, "{aggregate_line}").unwrap();
    drop(f);

    // Persist replacement records for two of the three tool_use_ids
    // (the third tool result stays at its original size).
    store
        .insert_content_replacement(
            SESSION,
            None,
            &[
                ContentReplacementRecord::tool_result(
                    "toolu_a",
                    "<persisted-output>alpha-shrunk</persisted-output>",
                ),
                ContentReplacementRecord::tool_result(
                    "toolu_c",
                    "<persisted-output>gamma-shrunk</persisted-output>",
                ),
            ],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");

    // All three tool_use_ids should be present as ToolResult messages
    // (parallel split via deterministic block uuids inside wire.rs).
    let tool_use_ids: HashSet<String> = state
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(tr) => Some(tr.tool_use_id.clone()),
            _ => None,
        })
        .collect();
    assert!(tool_use_ids.contains("toolu_a"));
    assert!(tool_use_ids.contains("toolu_b"));
    assert!(tool_use_ids.contains("toolu_c"));

    // Replacement records apply by tool_use_id regardless of how many
    // blocks shared the original parent user entry — no per-message
    // UUID filter.
    let by_id: HashSet<&str> = state
        .content_replacements
        .iter()
        .map(ContentReplacementRecord::tool_use_id)
        .collect();
    assert!(by_id.contains("toolu_a"));
    assert!(by_id.contains("toolu_c"));
    assert!(
        !by_id.contains("toolu_b"),
        "toolu_b had no replacement and must not surface one",
    );
}

// ---------------------------------------------------------------------------
// Recovery + replacement seeding round-trip
// ---------------------------------------------------------------------------

/// End-to-end: write a chain with two assistant turns and a couple of
/// content-replacement records, then run the resume loader and seed a
/// fresh `ContentReplacementState` from the result. The replacement
/// map MUST contain every persisted `tool_use_id`, proving the
/// snake_case wire deserialisation round-trips through the same code
/// path `SessionRuntime::seed_tool_result_replacement_state` uses.
#[test]
fn replacement_state_seeds_from_resume_state() {
    let (_dir, store, path) = fresh_store();
    let u1 = user_with_uuid(Uuid::new_v4(), "run a tool");
    let a1 = assistant_with_uuid(Uuid::new_v4(), "ran it");
    let u2 = user_with_uuid(Uuid::new_v4(), "and another");
    let a2 = assistant_with_uuid(Uuid::new_v4(), "ran another");
    write_chain(&store, &[u1, a1, u2, a2]);

    store
        .insert_content_replacement(
            SESSION,
            None,
            &[
                ContentReplacementRecord::tool_result("toolu_1", "<persisted>p1</persisted>"),
                ContentReplacementRecord::tool_result("toolu_2", "<persisted>p2</persisted>"),
            ],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");

    // Reseed the same way the runtime does on resume hydration.
    let records = &state.content_replacements;
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for rec in records {
        map.insert(rec.tool_use_id().to_string(), rec.replacement().to_string());
    }
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("toolu_1").unwrap(), "<persisted>p1</persisted>");
    assert_eq!(map.get("toolu_2").unwrap(), "<persisted>p2</persisted>");
}

// ---------------------------------------------------------------------------
// Combined torture: compact + tool_result + file_history + replacement
// ---------------------------------------------------------------------------

/// A realistic resume scenario: long-running session that hit a
/// compact boundary, has file-history snapshots both before and
/// after, a microcompact in the post-compact segment, and big
/// tool-result replacements scattered across both halves.
#[test]
fn combined_compact_microcompact_filehistory_and_replacements() {
    let (_dir, store, path) = fresh_store();
    // Pre-compact: u1 → a1 (tool) → ur1 (tool_result) → a2 (closing reply)
    // Compact boundary as a system entry
    // Post-compact: u3 → a3 (microcompact) → u4 → a4 (final)
    let u1 = Uuid::new_v4();
    let a1 = Uuid::new_v4();
    let a2 = Uuid::new_v4();
    let u3 = Uuid::new_v4();
    let a3 = Uuid::new_v4();
    let u4 = Uuid::new_v4();
    let a4 = Uuid::new_v4();

    write_chain(
        &store,
        &[
            user_with_uuid(u1, "kick off"),
            assistant_with_uuid(a1, "calling a tool"),
            assistant_with_uuid(a2, "summary of tool output"),
        ],
    );

    // Raw-append compact + post-compact segment. Use real Uuids
    // throughout — the read side parses entry uuids via `Uuid::parse`
    // and falls back to a random UUID on failure, which would break
    // any cross-reference (e.g. file-history-snapshot keyed on
    // outer message_id).
    let cb = Uuid::new_v4();
    let mc = Uuid::new_v4();
    let extras = vec![
        system_compact_boundary_line(
            &cb.to_string(),
            Some(&a2.to_string()),
            "2025-01-15T10:05:00Z",
        ),
        user_line(
            &u3.to_string(),
            Some(&cb.to_string()),
            "post-compact prompt",
            "2025-01-15T10:05:01Z",
        ),
        assistant_line(
            &a3.to_string(),
            &u3.to_string(),
            "post-compact reply",
            "2025-01-15T10:05:02Z",
        ),
        system_microcompact_boundary_line(&mc.to_string(), &a3.to_string(), "2025-01-15T10:05:03Z"),
        user_line(
            &u4.to_string(),
            Some(&mc.to_string()),
            "after mc",
            "2025-01-15T10:05:04Z",
        ),
        assistant_line(
            &a4.to_string(),
            &u4.to_string(),
            "final reply",
            "2025-01-15T10:05:05Z",
        ),
    ];
    use std::io::Write;
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        for line in extras {
            writeln!(f, "{line}").unwrap();
        }
    }

    // File-history snapshots: one before compact, one after.
    store
        .insert_file_history_snapshot(
            SESSION,
            &a1.to_string(),
            json!({"message_id": a1.to_string(), "v": "before"}),
            false,
        )
        .unwrap();
    store
        .insert_file_history_snapshot(
            SESSION,
            &u3.to_string(),
            json!({"message_id": u3.to_string(), "v": "after"}),
            false,
        )
        .unwrap();

    // Big replacements in both halves.
    let big_pre = "P".repeat(64 * 1024);
    let big_post = "Q".repeat(64 * 1024);
    store
        .insert_content_replacement(
            SESSION,
            None,
            &[
                ContentReplacementRecord::tool_result("toolu_pre", &big_pre),
                ContentReplacementRecord::tool_result("toolu_post", &big_post),
            ],
        )
        .unwrap();

    let state = load_session_state_for_resume(&path).expect("load");

    // Sanity: both pre- and post-compact turns surface.
    let user_texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => match &u.message {
                coco_messages::LlmMessage::User { content, .. } => {
                    content.iter().find_map(|c| match c {
                        coco_messages::UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        user_texts.iter().any(|t| t.contains("kick off")),
        "pre-compact user turn must survive (chain: {user_texts:?})",
    );
    assert!(
        user_texts.iter().any(|t| t.contains("after mc")),
        "post-microcompact user turn must surface in chain",
    );

    // Replacements: both records present, full payloads intact.
    let replacements: std::collections::HashMap<String, usize> = state
        .content_replacements
        .iter()
        .map(|r| (r.tool_use_id().to_string(), r.replacement().len()))
        .collect();
    assert_eq!(replacements.get("toolu_pre"), Some(&big_pre.len()));
    assert_eq!(replacements.get("toolu_post"), Some(&big_post.len()));

    // File-history chain: both snapshots replayed, in conversation order.
    let snapshot_versions: Vec<&str> = state
        .file_history_snapshots
        .iter()
        .map(|s| s.get("v").and_then(Value::as_str).unwrap_or(""))
        .collect();
    assert!(
        snapshot_versions.contains(&"before"),
        "pre-compact snapshot survives: {snapshot_versions:?}",
    );
    assert!(
        snapshot_versions.contains(&"after"),
        "post-compact snapshot present: {snapshot_versions:?}",
    );
}
