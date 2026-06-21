use super::*;
use serde_json::json;
use std::path::Path as StdPath;
use std::process::Command;

/// Build a [`TranscriptStore`] rooted at a fresh tempdir, returning
/// the tempdir guard (so it survives the test) and the resolved
/// project directory the store is actually writing to.
///
/// Production callers pass an `Arc<ProjectPaths>` derived from a
/// real cwd; tests use a synthetic project root so the slug math is
/// exercised without depending on the host filesystem layout.
fn test_store() -> (tempfile::TempDir, TranscriptStore, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Arc::new(coco_paths::ProjectPaths::new(
        dir.path().to_path_buf(),
        StdPath::new("/test-project"),
    ));
    let project_dir = paths.project_dir();
    let store = TranscriptStore::new(paths);
    (dir, store, project_dir)
}

fn run_git(dir: &StdPath, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[test]
fn usage_snapshot_writes_under_session_artifact_dir() {
    let (_dir, store, project_dir) = test_store();
    let snapshot = coco_types::SessionUsageSnapshot {
        version: 1,
        session_id: "sid-usage".into(),
        updated_at_ms: 42,
        totals: coco_types::SessionUsageTotals {
            input_tokens: 100,
            output_tokens: 25,
            total_cost_usd: 0.0042,
            request_count: 1,
            ..Default::default()
        },
        models: vec![coco_types::SessionModelUsageEntry {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-5".into(),
            input_tokens: 100,
            output_tokens: 25,
            total_cost_usd: 0.0042,
            request_count: 1,
            priced: true,
            ..Default::default()
        }],
        unpriced_models: Vec::new(),
    };

    store
        .write_usage_snapshot("sid-usage", &snapshot)
        .expect("usage snapshot should write");

    let path = project_dir.join("sid-usage").join("usage.json");
    assert!(path.exists());
    let loaded = store
        .load_usage_snapshot("sid-usage")
        .expect("usage snapshot should load")
        .expect("usage snapshot should exist");
    assert_eq!(loaded, snapshot);
}

#[test]
fn resolve_session_prefers_exact_worktree_slug_over_canonical_repo() {
    let repo = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let worktree_parent = tempfile::tempdir().unwrap();
    if !run_git(repo.path(), &["init"]) {
        return;
    }
    if !run_git(repo.path(), &["config", "user.email", "test@example.com"]) {
        return;
    }
    if !run_git(repo.path(), &["config", "user.name", "Test User"]) {
        return;
    }
    std::fs::write(repo.path().join("README.md"), "x").unwrap();
    if !run_git(repo.path(), &["add", "README.md"]) {
        return;
    }
    if !run_git(repo.path(), &["commit", "-m", "init"]) {
        return;
    }

    let worktree = worktree_parent.path().join("linked-worktree");
    if !run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "-b",
            "linked",
        ],
    ) {
        return;
    }

    let sid = "same-session";
    let repo_paths = ProjectPaths::new(memory.path().to_path_buf(), repo.path());
    let worktree_paths = ProjectPaths::new(memory.path().to_path_buf(), &worktree);
    std::fs::create_dir_all(repo_paths.project_dir()).unwrap();
    std::fs::create_dir_all(worktree_paths.project_dir()).unwrap();
    std::fs::write(
        repo_paths.project_dir().join(format!("{sid}.jsonl")),
        "repo\n",
    )
    .unwrap();
    std::fs::write(
        worktree_paths.project_dir().join(format!("{sid}.jsonl")),
        "worktree\n",
    )
    .unwrap();

    let resolved = resolve_session_file_path(memory.path(), sid, Some(&worktree))
        .unwrap()
        .expect("worktree transcript should resolve");
    assert_eq!(
        resolved.file_path,
        worktree_paths.project_dir().join(format!("{sid}.jsonl"))
    );
    assert_eq!(resolved.project_path.as_deref(), Some(worktree.as_path()));
}

#[test]
fn missing_usage_snapshot_loads_as_none() {
    let (_dir, store, _project_dir) = test_store();
    assert!(store.load_usage_snapshot("missing").unwrap().is_none());
}

#[test]
fn usage_snapshot_concurrent_writes_use_distinct_temp_files() {
    let (_dir, store, project_dir) = test_store();
    let store = Arc::new(store);
    let mut handles = Vec::new();
    for request_count in 1..=2 {
        let store = Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            let snapshot = coco_types::SessionUsageSnapshot {
                version: 1,
                session_id: "sid-race".into(),
                totals: coco_types::SessionUsageTotals {
                    request_count,
                    ..Default::default()
                },
                ..Default::default()
            };
            store.write_usage_snapshot("sid-race", &snapshot)
        }));
    }

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let loaded = store.load_usage_snapshot("sid-race").unwrap().unwrap();
    assert!((1..=2).contains(&loaded.totals.request_count));
    let session_dir = project_dir.join("sid-race");
    let leftover_tmp = std::fs::read_dir(session_dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"));
    assert!(!leftover_tmp);
}

fn make_user_entry(uuid: &str, session_id: &str, text: &str) -> TranscriptEntry {
    TranscriptEntry {
        entry_type: "user".to_string(),
        uuid: uuid.to_string(),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: session_id.to_string(),
        cwd: "/tmp/project".to_string(),
        timestamp: "2025-01-15T10:00:00Z".to_string(),
        version: Some("1.0.0".to_string()),
        git_branch: Some("main".to_string()),
        is_sidechain: false,
        agent_id: None,
        message: Some(json!({
            "role": "user",
            "content": text,
        })),
        usage: None,
        model: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    }
}

fn make_assistant_entry(uuid: &str, parent_uuid: &str, session_id: &str) -> TranscriptEntry {
    TranscriptEntry {
        entry_type: "assistant".to_string(),
        uuid: uuid.to_string(),
        parent_uuid: Some(parent_uuid.to_string()),
        logical_parent_uuid: None,
        session_id: session_id.to_string(),
        cwd: "/tmp/project".to_string(),
        timestamp: "2025-01-15T10:00:01Z".to_string(),
        version: Some("1.0.0".to_string()),
        git_branch: Some("main".to_string()),
        is_sidechain: false,
        agent_id: None,
        message: Some(json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "Sure, I can help."}],
        })),
        usage: Some(TranscriptUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        }),
        model: Some("claude-sonnet-4-6".into()),
        cost_usd: Some(0.003),
        extra: serde_json::Map::new(),
    }
}

#[test]
fn test_append_and_load_entries() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "test-session-001";

    let user = make_user_entry("u1", sid, "Hello world");
    let assistant = make_assistant_entry("a1", "u1", sid);

    store.append_message(sid, &user).unwrap();
    store.append_message(sid, &assistant).unwrap();

    let entries = store.load_entries(sid).unwrap();
    assert_eq!(entries.len(), 2);

    // Verify round-trip fidelity.
    match &entries[0] {
        Entry::Transcript(t) => {
            assert_eq!(t.entry_type, "user");
            assert_eq!(t.uuid, "u1");
        }
        other => panic!("expected Transcript, got {other:?}"),
    }
    match &entries[1] {
        Entry::Transcript(t) => {
            assert_eq!(t.entry_type, "assistant");
            assert_eq!(t.parent_uuid.as_deref(), Some("u1"));
        }
        other => panic!("expected Transcript, got {other:?}"),
    }
}

#[test]
fn test_metadata_entries_round_trip() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "meta-session";

    // Write a user message so the file has content.
    let user = make_user_entry("u1", sid, "Fix the bug");
    store.append_message(sid, &user).unwrap();

    // Append metadata entries.
    store
        .append_metadata(
            sid,
            &MetadataEntry::CustomTitle {
                session_id: sid.to_string(),
                custom_title: "Bug fix session".to_string(),
            },
        )
        .unwrap();
    store
        .append_metadata(
            sid,
            &MetadataEntry::Tag {
                session_id: sid.to_string(),
                tag: "bugfix".to_string(),
            },
        )
        .unwrap();
    store
        .append_metadata(
            sid,
            &MetadataEntry::LastPrompt {
                session_id: sid.to_string(),
                last_prompt: "Fix the bug".to_string(),
            },
        )
        .unwrap();

    // Load metadata.
    let meta = store.read_metadata(sid).unwrap();
    assert_eq!(meta.session_id, sid);
    assert_eq!(meta.custom_title.as_deref(), Some("Bug fix session"));
    assert_eq!(meta.tag.as_deref(), Some("bugfix"));
    assert_eq!(meta.last_prompt.as_deref(), Some("Fix the bug"));
    assert_eq!(meta.first_prompt, "Fix the bug");
    assert_eq!(meta.message_count, 1); // Only the user message.
    assert!(!meta.is_sidechain);
}

#[test]
fn test_content_replacement_records_round_trip() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "content-replacements";

    let records = vec![ContentReplacementRecord::tool_result(
        "toolu_1",
        "<persisted-output>\npreview\n</persisted-output>",
    )];
    store
        .insert_content_replacement(sid, /*agent_id*/ None, &records)
        .unwrap();

    assert_eq!(
        store.tool_results_session_dir(sid),
        _project_dir.join(sid).join("tool-results")
    );
    let loaded = store.load_content_replacements(sid).unwrap();
    assert_eq!(loaded, records);
}

#[test]
fn test_cleanup_tool_results_older_than_removes_expired_files_and_empty_dirs() {
    let (_dir, store, _project_dir) = test_store();
    let tool_results = store.tool_results_session_dir("session-a");
    let nested = tool_results.join("tool-dir");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(tool_results.join("call-1.txt"), "large output").unwrap();
    std::fs::write(nested.join("chunk-1.bin"), [1, 2, 3]).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(5));
    let removed = store
        .cleanup_tool_results_older_than(std::time::Duration::ZERO)
        .unwrap();

    assert_eq!(removed, 2);
    assert!(!tool_results.exists());
    assert!(!_project_dir.join("session-a").exists());
}

#[test]
fn test_cleanup_tool_results_older_than_keeps_recent_files() {
    let (_dir, store, _project_dir) = test_store();
    let tool_results = store.tool_results_session_dir("session-b");
    std::fs::create_dir_all(&tool_results).unwrap();
    let output = tool_results.join("call-1.txt");
    std::fs::write(&output, "large output").unwrap();

    let removed = store
        .cleanup_tool_results_older_than(std::time::Duration::from_secs(24 * 60 * 60))
        .unwrap();

    assert_eq!(removed, 0);
    assert!(output.exists());
    assert!(tool_results.exists());
}

#[test]
fn test_list_sessions_newest_first() {
    let (_dir, store, _project_dir) = test_store();

    // Create two sessions with a small delay so mtime differs.
    let user_a = make_user_entry("u1", "session-a", "First session");
    store.append_message("session-a", &user_a).unwrap();

    // Touch file to ensure different mtime.
    std::thread::sleep(std::time::Duration::from_millis(50));

    let user_b = make_user_entry("u2", "session-b", "Second session");
    store.append_message("session-b", &user_b).unwrap();

    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    // Newest first.
    assert_eq!(sessions[0].session_id, "session-b");
    assert_eq!(sessions[1].session_id, "session-a");
}

#[test]
fn test_load_transcript_messages_filters_metadata() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "filter-session";

    let user = make_user_entry("u1", sid, "Hello");
    store.append_message(sid, &user).unwrap();
    store
        .append_metadata(
            sid,
            &MetadataEntry::CustomTitle {
                session_id: sid.to_string(),
                custom_title: "Test".to_string(),
            },
        )
        .unwrap();
    let assistant = make_assistant_entry("a1", "u1", sid);
    store.append_message(sid, &assistant).unwrap();

    let messages = store.load_transcript_messages(sid).unwrap();
    // Only transcript messages, not metadata.
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].entry_type, "user");
    assert_eq!(messages[1].entry_type, "assistant");
}

#[test]
fn test_exists_and_delete() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "del-session";

    assert!(!store.exists(sid));

    let user = make_user_entry("u1", sid, "To be deleted");
    store.append_message(sid, &user).unwrap();
    assert!(store.exists(sid));

    store.delete(sid).unwrap();
    assert!(!store.exists(sid));
}

#[test]
fn test_extract_text_content_string() {
    let entry = make_user_entry("u1", "s1", "Simple text prompt");
    let text = extract_text_content(&entry);
    assert_eq!(text, "Simple text prompt");
}

#[test]
fn test_extract_text_content_array() {
    let mut entry = make_user_entry("u1", "s1", "");
    entry.message = Some(json!({
        "role": "user",
        "content": [
            {"type": "tool_result", "text": "ignored"},
            {"type": "text", "text": "Actual prompt text"},
        ],
    }));
    let text = extract_text_content(&entry);
    assert_eq!(text, "Actual prompt text");
}

#[test]
fn test_truncate_prompt_long_text() {
    let long = "a".repeat(300);
    let truncated = truncate_prompt(&long);
    assert!(truncated.ends_with("..."));
    // 200 chars + "..."
    assert_eq!(truncated.len(), 203);
}

#[test]
fn test_load_nonexistent_transcript() {
    let (_dir, store, _project_dir) = test_store();
    let result = store.load_entries("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_empty_and_malformed_lines_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.jsonl");
    std::fs::write(
        &path,
        "\n{\"not_a_valid_entry\": true}\n\n{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"s\",\"cwd\":\"\",\"timestamp\":\"\",\"isSidechain\":false}\n",
    )
    .unwrap();

    let entries = load_entries_from_file(&path).unwrap();
    // Empty lines skipped, malformed kept as Unknown, valid transcript parsed.
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0], Entry::Unknown(_)));
    assert!(matches!(&entries[1], Entry::Transcript(_)));
}

// ---------------------------------------------------------------------------
// Rewind support — file-history snapshot chain + marble-origami replay.
// These metadata entries persist into the JSONL alongside transcript
// messages. The rewind picker uses the snapshot chain to know which
// pre-edit file states it can restore to; marble-origami entries
// preserve context-collapse staging across resume.
// ---------------------------------------------------------------------------

#[test]
fn test_file_history_snapshot_round_trip_appends_in_order() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "rewind-session";

    // Seed transcript with at least one message so the JSONL has shape.
    store
        .append_message(sid, &make_user_entry("u1", sid, "edit a"))
        .unwrap();

    // Two distinct messages each get one file-history snapshot.
    store
        .insert_file_history_snapshot(
            sid,
            "msg-1",
            json!({"message_id": "msg-1", "files": {"a.txt": {"content": "v1"}}}),
            /*is_snapshot_update*/ false,
        )
        .unwrap();
    store
        .insert_file_history_snapshot(
            sid,
            "msg-2",
            json!({"message_id": "msg-2", "files": {"a.txt": {"content": "v2"}}}),
            false,
        )
        .unwrap();

    let chain_uuids: Vec<String> = vec!["msg-1".into(), "msg-2".into()];
    let chain = store
        .load_file_history_snapshots_for_chain(sid, &chain_uuids)
        .unwrap();
    assert_eq!(chain.len(), 2, "expected one snapshot per message_id");
    // Order matches the chain (= insertion order here since both msgs
    // are in the chain).
    assert_eq!(
        chain[0]
            .pointer("/files/a.txt/content")
            .and_then(|v| v.as_str()),
        Some("v1"),
    );
    assert_eq!(
        chain[1]
            .pointer("/files/a.txt/content")
            .and_then(|v| v.as_str()),
        Some("v2"),
    );
}

#[test]
fn test_file_history_snapshot_chain_last_wins_on_update() {
    // The rewind subsystem's `track_edit` flow rewrites a not-yet-
    // flushed snapshot in place — `is_snapshot_update = true` should
    // overwrite the prior entry for the same INNER `snapshot.messageId`
    // rather than appending a new row.
    //
    // The update entry's outer `message_id` (the current turn's id)
    // differs from the inner `snapshot.messageId` (the original
    // snapshot's id); the builder keys on the INNER field.
    let (_dir, store, _project_dir) = test_store();
    let sid = "update-session";
    store
        .append_message(sid, &make_user_entry("u1", sid, "edit"))
        .unwrap();

    store
        .insert_file_history_snapshot(
            sid,
            "msg-A",
            json!({"message_id": "msg-A", "version": "first"}),
            false,
        )
        .unwrap();
    store
        .insert_file_history_snapshot(
            sid,
            "msg-B",
            json!({"message_id": "msg-B", "version": "first-B"}),
            false,
        )
        .unwrap();
    // Update for msg-A: outer message_id is the *current* turn
    // ("msg-update-turn") but the inner snapshot.messageId is still
    // "msg-A" — TS `recordFileHistorySnapshot(messageId, snapshot,
    // true)` always sets the outer to `messageId` while the inner
    // tracks the snapshot it overwrites.
    store
        .insert_file_history_snapshot(
            sid,
            "msg-update-turn",
            json!({"message_id": "msg-A", "version": "second"}),
            /*is_snapshot_update*/ true,
        )
        .unwrap();

    let chain_uuids: Vec<String> = vec!["msg-A".into(), "msg-B".into(), "msg-update-turn".into()];
    let chain = store
        .load_file_history_snapshots_for_chain(sid, &chain_uuids)
        .unwrap();
    assert_eq!(
        chain.len(),
        2,
        "update should overwrite, not append (got {chain:?})",
    );
    // msg-A's slot now holds the updated snapshot…
    assert_eq!(
        chain[0].pointer("/version").and_then(|v| v.as_str()),
        Some("second"),
    );
    // …and msg-B is unchanged at its position.
    assert_eq!(
        chain[1].pointer("/version").and_then(|v| v.as_str()),
        Some("first-B"),
    );
}

#[test]
fn test_file_history_snapshot_chain_unknown_id_ignored() {
    // An update whose inner messageId has not been recorded yet should
    // be treated as a fresh insert (`existingIndex === undefined` falls
    // through to push). The chain walk uses the conversation-ordered
    // UUID list — entries for ids absent from the chain are silently
    // skipped.
    let entries = vec![
        Entry::Metadata(MetadataEntry::FileHistorySnapshot {
            message_id: "msg-A".into(),
            snapshot: json!({"message_id": "msg-A", "v": "a1"}),
            is_snapshot_update: false,
        }),
        // is_snapshot_update=true for an *unknown* inner id — falls
        // through and gets pushed as a new entry.
        Entry::Metadata(MetadataEntry::FileHistorySnapshot {
            message_id: "msg-Z".into(),
            snapshot: json!({"message_id": "msg-Z", "v": "z-update"}),
            is_snapshot_update: true,
        }),
    ];
    let chain_uuids: Vec<String> = vec!["msg-A".into(), "msg-Z".into()];
    let chain = build_file_history_snapshot_chain(&entries, &chain_uuids);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].pointer("/v").and_then(|v| v.as_str()), Some("a1"));
    assert_eq!(
        chain[1].pointer("/v").and_then(|v| v.as_str()),
        Some("z-update"),
    );
}

#[test]
fn test_marble_origami_entries_filtered_by_session() {
    // Both commit and snapshot entries are session-scoped via the
    // `sessionId` field on the payload. The loader must drop entries
    // tagged with a different session_id (e.g. when transcripts get
    // forked / merged).
    let (_dir, store, _project_dir) = test_store();
    let sid = "mo-session";
    store
        .append_message(sid, &make_user_entry("u1", sid, "p"))
        .unwrap();

    // Two commits for our session, one for a different session.
    store
        .append_marble_origami_commit(sid, json!({"session_id": sid, "id": "c1"}))
        .unwrap();
    store
        .append_marble_origami_commit(sid, json!({"session_id": "other", "id": "c-other"}))
        .unwrap();
    store
        .append_marble_origami_commit(sid, json!({"session_id": sid, "id": "c2"}))
        .unwrap();

    // Two snapshots — last-wins by sessionId; only the our-session one counts.
    store
        .append_marble_origami_snapshot(sid, json!({"session_id": "other", "v": "ignore"}))
        .unwrap();
    store
        .append_marble_origami_snapshot(sid, json!({"session_id": sid, "v": "keep-1"}))
        .unwrap();
    store
        .append_marble_origami_snapshot(sid, json!({"session_id": sid, "v": "keep-2"}))
        .unwrap();

    let (commits, snapshot) = store.load_marble_origami_entries(sid).unwrap();
    assert_eq!(commits.len(), 2, "off-session commit must be dropped");
    assert_eq!(
        commits[0].pointer("/id").and_then(|v| v.as_str()),
        Some("c1")
    );
    assert_eq!(
        commits[1].pointer("/id").and_then(|v| v.as_str()),
        Some("c2")
    );

    let snap = snapshot.expect("snapshot should exist");
    assert_eq!(
        snap.pointer("/v").and_then(|v| v.as_str()),
        Some("keep-2"),
        "last-wins on snapshot for matching session",
    );
}

/// Wire-format regression: transcript JSONL is snake_case JSON.
/// No `serde(rename_all = "camelCase")` on the wire types.
#[test]
fn test_transcript_entry_serializes_with_snake_case_keys() {
    let entry = TranscriptEntry {
        entry_type: "assistant".into(),
        uuid: "uu".into(),
        parent_uuid: Some("pp".into()),
        logical_parent_uuid: None,
        session_id: "ss".into(),
        cwd: "/tmp".into(),
        timestamp: "2025-01-15T10:00:00Z".into(),
        version: Some("1.0".into()),
        git_branch: Some("main".into()),
        is_sidechain: true,
        agent_id: None,
        message: Some(json!({"role": "assistant", "content": []})),
        usage: Some(TranscriptUsage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: Some(3),
            cache_creation_tokens: Some(4),
        }),
        model: Some("claude-sonnet-4-6".into()),
        cost_usd: Some(0.5),
        extra: serde_json::Map::new(),
    };
    let v = serde_json::to_value(&entry).unwrap();
    // Snake_case wire. See storage.rs module doc.
    assert!(v.get("parent_uuid").is_some());
    assert!(v.get("session_id").is_some());
    assert!(v.get("is_sidechain").is_some());
    assert!(v.get("git_branch").is_some());
    assert!(v.get("cost_usd").is_some());
    assert!(v.get("parentUuid").is_none());
    assert!(v.get("sessionId").is_none());
    assert!(v.get("isSidechain").is_none());
    let usage = v.get("usage").expect("usage present");
    assert!(usage.get("input_tokens").is_some());
    assert!(usage.get("output_tokens").is_some());
    assert!(usage.get("cache_read_tokens").is_some());
    assert!(usage.get("cache_creation_tokens").is_some());
    assert!(usage.get("inputTokens").is_none());
}

/// MetadataEntry uses a kebab-case `type:` discriminator (matches TS
/// semantic taxonomy) but payload field names are Rust snake_case.
#[test]
fn test_metadata_entry_serializes_with_snake_case_payload() {
    let m = MetadataEntry::CustomTitle {
        session_id: "ss".into(),
        custom_title: "My Bug Hunt".into(),
    };
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("custom-title"));
    assert_eq!(v.get("session_id").and_then(|t| t.as_str()), Some("ss"));
    assert_eq!(
        v.get("custom_title").and_then(|t| t.as_str()),
        Some("My Bug Hunt"),
    );
    assert!(v.get("sessionId").is_none());
    assert!(v.get("customTitle").is_none());
}

#[test]
fn test_content_replacement_serializes_ts_shape() {
    // Three fields per record — `kind`, `tool_use_id`, `replacement`.
    // No `message_uuid` — records are matched on `tool_use_id` only.
    let m = MetadataEntry::ContentReplacement {
        session_id: "ss".into(),
        agent_id: None,
        replacements: vec![ContentReplacementRecord::tool_result(
            "toolu_1",
            "replacement",
        )],
    };
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(
        v.get("type").and_then(|t| t.as_str()),
        Some("content-replacement")
    );
    assert_eq!(v.get("session_id").and_then(|t| t.as_str()), Some("ss"));
    let replacements = v
        .get("replacements")
        .and_then(|t| t.as_array())
        .expect("replacements array");
    assert_eq!(replacements.len(), 1);
    assert_eq!(
        replacements[0].get("kind").and_then(|t| t.as_str()),
        Some("tool-result")
    );
    assert!(
        replacements[0].get("message_uuid").is_none(),
        "message_uuid must not be present — records key on tool_use_id"
    );
    assert_eq!(
        replacements[0].get("tool_use_id").and_then(|t| t.as_str()),
        Some("toolu_1")
    );
    assert_eq!(
        replacements[0].get("replacement").and_then(|t| t.as_str()),
        Some("replacement")
    );
}

#[test]
fn test_tool_result_transcript_serializes_as_user_message_with_tool_result_block() {
    let tool_result = coco_messages::create_tool_result_message(
        "toolu_1",
        "Read",
        coco_types::ToolId::Custom("Read".into()),
        "file contents",
        false,
    );
    let entries = transcript_entries_for_message(
        &tool_result,
        TranscriptEntryOptions {
            session_id: "ss",
            cwd: "/tmp",
            timestamp: "2025-01-15T10:00:00Z",
            parent_uuid: Some("assistant-uuid"),
            logical_parent_uuid: None,
            is_sidechain: false,
            agent_id: None,
            git_branch: None,
        },
    );
    assert_eq!(entries.len(), 1);
    let v = serde_json::to_value(&entries[0]).unwrap();
    assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("user"));
    assert_ne!(v.get("type").and_then(|t| t.as_str()), Some("tool_result"));
    let content = v
        .pointer("/message/content")
        .and_then(|t| t.as_array())
        .expect("content blocks");
    assert_eq!(content.len(), 1);
    assert_eq!(
        content[0].get("type").and_then(|t| t.as_str()),
        Some("tool_result")
    );
    assert_eq!(
        content[0].get("tool_use_id").and_then(|t| t.as_str()),
        Some("toolu_1")
    );
    assert_eq!(
        content[0].get("content").and_then(|t| t.as_str()),
        Some("file contents")
    );
}

#[test]
fn test_transcript_only_flag_survives_jsonl_round_trip() {
    // Regression guard: a slash-command result is transcript-only (never
    // sent to the model). If the flag is dropped on persist, it resumes as
    // a model-visible user message — an API leak on resume. Round-trip it
    // and assert the gate survives.
    let messages = coco_messages::build_slash_command_messages("model", "", "Set Main → x", false);
    let coco_messages::Message::User(original) = &messages[1] else {
        panic!("expected user result message");
    };
    assert!(original.is_visible_in_transcript_only);

    let entries = transcript_entries_for_message(
        &messages[1],
        TranscriptEntryOptions {
            session_id: "ss",
            cwd: "/tmp",
            timestamp: "2025-01-15T10:00:00Z",
            parent_uuid: None,
            logical_parent_uuid: None,
            is_sidechain: false,
            agent_id: None,
            git_branch: None,
        },
    );
    assert_eq!(entries.len(), 1);
    let restored = messages_from_transcript_entry(&entries[0]);
    assert_eq!(restored.len(), 1);
    let coco_messages::Message::User(restored) = &restored[0] else {
        panic!("expected restored user message");
    };
    assert!(
        restored.is_visible_in_transcript_only,
        "transcript-only gate must survive resume or the model sees it"
    );
    assert_eq!(
        restored.origin,
        Some(coco_messages::MessageOrigin::SlashCommand)
    );
}

#[test]
fn test_append_message_chain_parents_tool_result_to_source_assistant() {
    let (_dir, store, _project_dir) = test_store();
    let sid = "source-parent";
    let user = coco_messages::create_user_message("read it");
    let assistant = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::ToolCall(
            coco_messages::ToolCallContent::new(
                "toolu_1".to_string(),
                "Read".to_string(),
                json!({"file_path": "a.txt"}),
            ),
        )],
        "mock",
        coco_types::TokenUsage::default(),
    );
    let assistant_uuid = assistant.uuid().copied().unwrap();
    let hook_message = coco_messages::create_user_message("hook inserted context");
    let tool_result = coco_messages::create_tool_result_message(
        "toolu_1",
        "Read",
        coco_types::ToolId::Custom("Read".into()),
        "file contents",
        false,
    );
    let tool_result_uuid = tool_result.uuid().copied().unwrap();
    let messages = [user, assistant, hook_message, tool_result];
    let mut seen = std::collections::HashSet::new();

    store
        .append_message_chain(
            sid,
            messages.iter(),
            &mut seen,
            ChainWriteOptions {
                cwd: "/tmp".into(),
                timestamp: "2025-01-15T10:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();

    let entries = store.load_transcript_messages(sid).unwrap();
    let persisted_tool_result = entries
        .iter()
        .find(|entry| entry.uuid == tool_result_uuid.to_string())
        .expect("tool result persisted");
    assert_eq!(
        persisted_tool_result.parent_uuid,
        Some(assistant_uuid.to_string())
    );
}

#[test]
fn test_replay_metadata_filters_to_selected_chain_and_agent() {
    // File-history snapshots key by the message the snapshot was
    // attached to; the chain walk respects conversation order so
    // entries for messages outside the chain are skipped.
    // Content-replacement records key by `tool_use_id` only and are
    // routed purely by `agent_id` presence — no per-message scope.
    let chain_uuids: Vec<String> = vec!["msg-current".into()];
    let entries = vec![
        Entry::Metadata(MetadataEntry::FileHistorySnapshot {
            message_id: "msg-stale".into(),
            snapshot: json!({"message_id": "msg-stale", "v": "stale"}),
            is_snapshot_update: false,
        }),
        Entry::Metadata(MetadataEntry::FileHistorySnapshot {
            message_id: "msg-current".into(),
            snapshot: json!({"message_id": "msg-current", "v": "current"}),
            is_snapshot_update: false,
        }),
        Entry::Metadata(MetadataEntry::ContentReplacement {
            session_id: "s".into(),
            agent_id: None,
            replacements: vec![
                ContentReplacementRecord::tool_result("toolu_1", "stale"),
                ContentReplacementRecord::tool_result("toolu_1", "current"),
            ],
        }),
        Entry::Metadata(MetadataEntry::ContentReplacement {
            session_id: "s".into(),
            agent_id: Some("agent-a".into()),
            replacements: vec![ContentReplacementRecord::tool_result("toolu_1", "agent")],
        }),
    ];

    let snapshots = build_file_history_snapshot_chain(&entries, &chain_uuids);
    assert_eq!(
        snapshots,
        vec![json!({"message_id": "msg-current", "v": "current"})]
    );

    // Main-thread (agent_id=None) returns both records — caller
    // applies-in-order so the later "current" overrides the earlier
    // "stale" by tool_use_id when seeding ContentReplacementState.
    let main_replacements = content_replacements_for_chain(&entries, "s", None);
    assert_eq!(main_replacements.len(), 2);
    assert_eq!(main_replacements[0].replacement(), "stale");
    assert_eq!(main_replacements[1].replacement(), "current");

    let agent_replacements = content_replacements_for_chain(&entries, "s", Some("agent-a"));
    assert_eq!(agent_replacements.len(), 1);
    assert_eq!(agent_replacements[0].replacement(), "agent");
}
