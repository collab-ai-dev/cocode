use crate::SessionManager;
use crate::storage::TranscriptEntry;
use crate::storage::TranscriptStore;
use std::path::Path;
use std::sync::Arc;

/// Synthetic cwd every test in this module uses. Tests write JSONL
/// fixtures under the slug coco-paths produces for this path so the
/// `SessionManager` global scan can locate them.
const TEST_CWD: &str = "/test-cwd";

fn project_paths(memory_base: &Path) -> Arc<coco_paths::ProjectPaths> {
    Arc::new(coco_paths::ProjectPaths::new(
        memory_base.to_path_buf(),
        Path::new(TEST_CWD),
    ))
}

fn seed_transcript(memory_base: &Path, sid: &str) -> Arc<coco_paths::ProjectPaths> {
    let paths = project_paths(memory_base);
    let store = TranscriptStore::new(paths.clone());
    let entry = TranscriptEntry {
        entry_type: "user".to_string(),
        uuid: format!("{sid}-u1"),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: sid.to_string(),
        cwd: TEST_CWD.to_string(),
        timestamp: "2025-01-15T10:00:00Z".to_string(),
        version: Some("1.0.0".to_string()),
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "hi"}],
        })),
        usage: None,
        model: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    store.append_message(sid, &entry).unwrap();
    paths
}

#[test]
fn create_returns_in_memory_session_without_writing_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let session = mgr.create("test-model", Path::new("/tmp")).unwrap();
    assert_eq!(session.model, "test-model");
    assert!(!session.id.is_empty());
    // `create` is in-memory only. Nothing on disk yet.
    assert!(mgr.load(&session.id).is_err());
}

#[test]
fn load_derives_session_from_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-alpha");
    let loaded = mgr.load("sess-alpha").unwrap();
    assert_eq!(loaded.id, "sess-alpha");
    assert_eq!(loaded.working_dir, std::path::PathBuf::from(TEST_CWD));
}

#[test]
fn list_walks_every_project() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-a");
    seed_transcript(dir.path(), "sess-b");
    let list = mgr.list().unwrap();
    assert_eq!(list.len(), 2);
}

#[test]
fn resume_is_equivalent_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-r");
    let resumed = mgr.resume("sess-r").unwrap();
    assert_eq!(resumed.id, "sess-r");
}

#[test]
fn delete_removes_jsonl_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-d");
    assert!(paths.transcript("sess-d").exists());
    mgr.delete("sess-d").unwrap();
    assert!(!paths.transcript("sess-d").exists());
    assert!(mgr.load("sess-d").is_err());
}

#[test]
fn most_recent_returns_newest_session() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    assert!(mgr.most_recent().unwrap().is_none());
    seed_transcript(dir.path(), "sess-x");
    assert!(mgr.most_recent().unwrap().is_some());
}

#[test]
fn set_title_appends_custom_title_and_agent_name_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-t");
    let session = mgr.set_title("sess-t", "my session").unwrap();
    assert_eq!(session.title.as_deref(), Some("my session"));

    // Re-derive and confirm the title is read back from the JSONL.
    let loaded = mgr.load("sess-t").unwrap();
    assert_eq!(loaded.title.as_deref(), Some("my session"));
    let raw = std::fs::read_to_string(paths.transcript("sess-t")).unwrap();
    // rename writes BOTH `custom-title` (picker) AND
    // `agent-name` (prompt-bar banner) in the same transcript pass.
    assert!(
        raw.contains("\"custom-title\""),
        "expected custom-title metadata entry: {raw}",
    );
    assert!(
        raw.contains("\"agent-name\""),
        "expected agent-name metadata entry: {raw}",
    );
}

#[test]
fn save_mode_appends_mode_metadata_for_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-mode");
    mgr.save_mode("sess-mode", "coordinator").unwrap();
    let raw = std::fs::read_to_string(paths.transcript("sess-mode")).unwrap();
    // A `mode` metadata entry records the session's coordinator state
    // so `reconcile_on_resume` can re-derive it.
    assert!(
        raw.contains("\"mode\""),
        "expected mode metadata entry: {raw}",
    );
    assert!(
        raw.contains("coordinator"),
        "expected the coordinator mode value: {raw}",
    );
}

#[test]
fn set_ai_title_writes_ai_title_entry_distinct_from_custom_title() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-ai");
    let session = mgr.set_ai_title("sess-ai", "ai-suggested").unwrap();
    // Read-precedence: ai title fills the slot only when no
    // CustomTitle exists, but the in-memory `Session` returned by
    // `set_ai_title` reflects the new value either way.
    assert_eq!(session.title.as_deref(), Some("ai-suggested"));

    let raw = std::fs::read_to_string(paths.transcript("sess-ai")).unwrap();
    assert!(
        raw.contains("\"ai-title\""),
        "expected ai-title entry in transcript: {raw}",
    );
    assert!(
        !raw.contains("\"custom-title\""),
        "set_ai_title must not write custom-title: {raw}",
    );
    assert!(
        !raw.contains("\"agent-name\""),
        "set_ai_title must not write agent-name: {raw}",
    );
}

#[test]
fn user_set_title_wins_over_prior_ai_title_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-mix");

    // AI suggests first, then user renames. Reader prefers
    // CustomTitle regardless of file order.
    mgr.set_ai_title("sess-mix", "auto-name").unwrap();
    mgr.set_title("sess-mix", "user-chosen").unwrap();
    let loaded = mgr.load("sess-mix").unwrap();
    assert_eq!(loaded.title.as_deref(), Some("user-chosen"));
}

#[test]
fn ai_title_arriving_after_user_rename_does_not_clobber_on_read() {
    // Reversed write order: user renames first, then a stale AI
    // title lands. The user title must still win on read.
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-race");

    mgr.set_title("sess-race", "user-chosen").unwrap();
    mgr.set_ai_title("sess-race", "auto-name").unwrap();
    let loaded = mgr.load("sess-race").unwrap();
    assert_eq!(loaded.title.as_deref(), Some("user-chosen"));
}

#[test]
fn set_ai_title_replaces_prior_ai_title_in_returned_session() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-ai-replace");

    mgr.set_ai_title("sess-ai-replace", "old-auto").unwrap();
    let session = mgr.set_ai_title("sess-ai-replace", "new-auto").unwrap();

    assert_eq!(session.title.as_deref(), Some("new-auto"));
    let loaded = mgr.load("sess-ai-replace").unwrap();
    assert_eq!(loaded.title.as_deref(), Some("new-auto"));
}

#[test]
fn re_append_session_metadata_writes_custom_title_and_agent_name_to_eof() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-r1");
    mgr.set_title("sess-r1", "fix-login").unwrap();
    // Count the two metadata kinds before re-append.
    let raw_before = std::fs::read_to_string(paths.transcript("sess-r1")).unwrap();
    let custom_before = raw_before.matches("\"custom-title\"").count();
    let agent_before = raw_before.matches("\"agent-name\"").count();

    mgr.re_append_session_metadata("sess-r1").unwrap();

    let raw_after = std::fs::read_to_string(paths.transcript("sess-r1")).unwrap();
    assert_eq!(
        raw_after.matches("\"custom-title\"").count(),
        custom_before + 1,
        "expected exactly one extra custom-title entry: {raw_after}",
    );
    assert_eq!(
        raw_after.matches("\"agent-name\"").count(),
        agent_before + 1,
        "expected exactly one extra agent-name entry (paired with custom-title): {raw_after}",
    );
}

#[test]
fn re_append_session_metadata_does_not_reappend_ai_title() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-ai-r");
    mgr.set_ai_title("sess-ai-r", "auto-name").unwrap();
    let raw_before = std::fs::read_to_string(paths.transcript("sess-ai-r")).unwrap();
    let ai_before = raw_before.matches("\"ai-title\"").count();

    mgr.re_append_session_metadata("sess-ai-r").unwrap();

    let raw_after = std::fs::read_to_string(paths.transcript("sess-ai-r")).unwrap();
    assert_eq!(raw_after.matches("\"ai-title\"").count(), ai_before);
    assert!(
        !raw_after.contains("\"custom-title\""),
        "AI title must not be re-appended as custom-title: {raw_after}",
    );
}

#[test]
fn re_append_session_metadata_noop_when_no_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    // No `seed_transcript` — the session has no JSONL on disk yet.
    // re_append_session_metadata bails when no transcript exists.
    assert!(mgr.re_append_session_metadata("sess-missing").is_ok());
}

#[test]
fn re_append_session_metadata_skips_when_no_title_or_tag() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-bare");
    // No set_title, no toggle_tag — re-append must not invent any
    // entries.
    let before = std::fs::read_to_string(paths.transcript("sess-bare")).unwrap();
    mgr.re_append_session_metadata("sess-bare").unwrap();
    let after = std::fs::read_to_string(paths.transcript("sess-bare")).unwrap();
    assert_eq!(before, after);
}

#[test]
fn re_append_session_metadata_preserves_tag() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-tag");
    mgr.toggle_tag("sess-tag", "wip").unwrap();
    let raw_before = std::fs::read_to_string(paths.transcript("sess-tag")).unwrap();
    // Match on the discriminator (`"type":"tag"`) rather than the
    // bare `"tag"` substring — the value field uses the same key
    // name so substring matches double-count.
    let tags_before = raw_before.matches(r#""type":"tag""#).count();

    mgr.re_append_session_metadata("sess-tag").unwrap();

    let raw_after = std::fs::read_to_string(paths.transcript("sess-tag")).unwrap();
    assert_eq!(
        raw_after.matches(r#""type":"tag""#).count(),
        tags_before + 1
    );
}

#[test]
fn re_append_session_metadata_preserves_ts_metadata_slots() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "sess-meta");
    let store = TranscriptStore::new(paths.clone());
    let sid = "sess-meta".to_string();
    let entries = [
        crate::storage::MetadataEntry::LastPrompt {
            session_id: sid.clone(),
            last_prompt: "fix failing tests".to_string(),
        },
        crate::storage::MetadataEntry::AgentColor {
            session_id: sid.clone(),
            agent_color: "blue".to_string(),
        },
        crate::storage::MetadataEntry::AgentSetting {
            session_id: sid.clone(),
            agent_setting: "reviewer".to_string(),
        },
        crate::storage::MetadataEntry::Mode {
            session_id: sid.clone(),
            mode: "coordinator".to_string(),
        },
        crate::storage::MetadataEntry::WorktreeState {
            payload: serde_json::json!({
                "session_id": sid,
                "worktree_session": false,
            }),
        },
        crate::storage::MetadataEntry::PrLink {
            payload: serde_json::json!({
                "session_id": "sess-meta",
                "pr_number": 42,
                "pr_url": "https://github.com/example/repo/pull/42",
                "pr_repository": "example/repo",
                "timestamp": "2025-01-15T10:00:00Z",
            }),
        },
    ];
    for entry in entries {
        store.append_metadata("sess-meta", &entry).unwrap();
    }
    let raw_before = std::fs::read_to_string(paths.transcript("sess-meta")).unwrap();

    mgr.re_append_session_metadata("sess-meta").unwrap();

    let raw_after = std::fs::read_to_string(paths.transcript("sess-meta")).unwrap();
    for kind in [
        r#""type":"last-prompt""#,
        r#""type":"agent-color""#,
        r#""type":"agent-setting""#,
        r#""type":"mode""#,
        r#""type":"worktree-state""#,
        r#""type":"pr-link""#,
    ] {
        assert_eq!(
            raw_after.matches(kind).count(),
            raw_before.matches(kind).count() + 1,
            "expected exactly one extra {kind} entry: {raw_after}",
        );
    }
}

#[test]
fn find_by_title_case_insensitive_substring() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-fix");
    seed_transcript(dir.path(), "sess-add");
    seed_transcript(dir.path(), "sess-other");
    mgr.set_title("sess-fix", "Fix login bug").unwrap();
    mgr.set_title("sess-add", "add-auth-feature").unwrap();
    mgr.set_title("sess-other", "Refactor API client").unwrap();

    // Substring + case-insensitive.
    let matches = mgr.find_by_title("LOGIN", false).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "sess-fix");

    let matches = mgr.find_by_title("fix", false).unwrap();
    assert_eq!(matches.len(), 1);

    // Token shared by two titles → multi-match.
    let multi = mgr.find_by_title("a", false).unwrap();
    assert!(multi.len() >= 2);
}

#[test]
fn find_by_title_exact_mode_rejects_substring_match() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-1");
    mgr.set_title("sess-1", "Fix login bug").unwrap();

    assert!(mgr.find_by_title("login", true).unwrap().is_empty());
    assert_eq!(mgr.find_by_title("fix login bug", true).unwrap().len(), 1);
    assert_eq!(mgr.find_by_title("FIX LOGIN BUG", true).unwrap().len(), 1);
}

#[test]
fn find_by_title_does_not_match_ai_title() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-ai-search");
    mgr.set_ai_title("sess-ai-search", "auto search title")
        .unwrap();

    assert!(
        mgr.find_by_title("auto search title", true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn find_by_title_empty_query_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "sess-1");
    mgr.set_title("sess-1", "Anything").unwrap();
    assert!(mgr.find_by_title("", false).unwrap().is_empty());
    assert!(mgr.find_by_title("   ", false).unwrap().is_empty());
}

#[test]
fn find_by_title_skips_untitled_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    seed_transcript(dir.path(), "untitled");
    let matches = mgr.find_by_title("anything", false).unwrap();
    assert!(matches.is_empty());
}

#[test]
fn cleanup_older_than_unlinks_stale_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let paths = seed_transcript(dir.path(), "old");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let removed = mgr.cleanup_older_than(std::time::Duration::ZERO).unwrap();
    assert_eq!(removed, 1);
    assert!(!paths.transcript("old").exists());
}
