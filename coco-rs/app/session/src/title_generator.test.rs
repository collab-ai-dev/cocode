//! Tests for session title auto-generation.

use super::*;

#[test]
fn prompt_includes_plan_body() {
    let (_sys, user) = build_title_prompt("# Fix auth\n- add middleware");
    assert!(user.contains("Fix auth"));
    assert!(user.contains("add middleware"));
    assert!(user.contains("--- PLAN ---"));
}

#[test]
fn prompt_truncates_long_plans_safely() {
    let plan = "x".repeat(5_000);
    let (_sys, user) = build_title_prompt(&plan);
    // Truncation cap is PLAN_CONTEXT_CHARS=1000. The body + label
    // overhead is bounded; just verify we didn't dump the full 5k.
    assert!(user.len() < 1500);
}

#[test]
fn prompt_is_char_boundary_safe() {
    // Build a string whose cut-point would split a multi-byte char if
    // we did naive byte slicing. With PLAN_CONTEXT_CHARS=1000, we place
    // a 2-byte emoji right at byte 999 (non-boundary) and 1000 (emoji
    // start) to exercise the backtracking logic.
    let mut plan = "a".repeat(998);
    plan.push('é'); // 2-byte UTF-8
    plan.push_str(&"b".repeat(3000));
    // Must not panic.
    let (_sys, user) = build_title_prompt(&plan);
    assert!(user.contains("--- PLAN ---"));
}

#[test]
fn parse_strict_json_object() {
    assert_eq!(
        parse_title_response(r#"{"title":"Fix login button"}"#),
        Some("Fix login button".into())
    );
}

#[test]
fn parse_json_object_with_whitespace() {
    assert_eq!(
        parse_title_response("  \n { \"title\" : \"Update docs\" }  "),
        Some("Update docs".into())
    );
}

#[test]
fn parse_bare_json_string_fallback() {
    assert_eq!(
        parse_title_response("\"Refactor auth flow\""),
        Some("Refactor auth flow".into())
    );
}

#[test]
fn parse_plaintext_response_fallback() {
    // Model ignored schema — extract first line.
    assert_eq!(
        parse_title_response("Fix the dashboard crash"),
        Some("Fix the dashboard crash".into())
    );
}

#[test]
fn parse_plaintext_strips_title_label() {
    assert_eq!(
        parse_title_response("title: Add caching"),
        Some("Add caching".into())
    );
    assert_eq!(
        parse_title_response("Title: Add caching"),
        Some("Add caching".into())
    );
}

#[test]
fn parse_strips_trailing_punctuation() {
    assert_eq!(
        parse_title_response(r#"{"title":"Fix bug!"}"#),
        Some("Fix bug".into())
    );
    assert_eq!(
        parse_title_response(r#"{"title":"Fix bug."}"#),
        Some("Fix bug".into())
    );
}

#[test]
fn parse_rejects_too_short() {
    assert_eq!(parse_title_response(r#"{"title":"OK"}"#), None);
    assert_eq!(parse_title_response("  "), None);
    assert_eq!(parse_title_response(""), None);
}

#[test]
fn parse_caps_very_long_titles() {
    let long = "x".repeat(500);
    let raw = format!(r#"{{"title":"{long}"}}"#);
    let parsed = parse_title_response(&raw).unwrap();
    assert!(parsed.len() <= 100);
}

/// Seed a JSONL transcript so the JSONL-canonical `SessionManager`
/// (post-fix) can locate and derive a `Session` for the id.
fn seed_transcript(memory_base: &std::path::Path, sid: &str) {
    use crate::storage::TranscriptEntry;
    use crate::storage::TranscriptStore;
    let paths = std::sync::Arc::new(coco_paths::ProjectPaths::new(
        memory_base.to_path_buf(),
        std::path::Path::new("/test-cwd"),
    ));
    let store = TranscriptStore::new(paths);
    let entry = TranscriptEntry {
        entry_type: "user".to_string(),
        uuid: format!("{sid}-u1"),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: sid.to_string(),
        cwd: "/test-cwd".to_string(),
        timestamp: "2025-01-15T10:00:00Z".to_string(),
        version: None,
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(serde_json::json!({"role":"user","content":"hi"})),
        usage: None,
        model: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    store.append_message(sid, &entry).unwrap();
}

#[test]
fn apply_title_persists_when_session_has_none() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = crate::SessionManager::new(tmp.path().to_path_buf());
    seed_transcript(tmp.path(), "sess-a");
    let s = mgr.load("sess-a").unwrap();
    assert!(s.title.is_none());

    let applied = apply_title(&mgr, &s.id, "Fix login button".into()).unwrap();
    assert!(applied, "expected freshly-titled session");

    let reloaded = mgr.load(&s.id).unwrap();
    assert_eq!(reloaded.title.as_deref(), Some("Fix login button"));
    // AI-generated titles are persisted via `MetadataEntry::AiTitle`,
    // never `CustomTitle` — that is the schema invariant that lets a
    // subsequent user `/rename` always win on read.
    let raw = std::fs::read_to_string(
        std::sync::Arc::new(coco_paths::ProjectPaths::new(
            tmp.path().to_path_buf(),
            std::path::Path::new("/test-cwd"),
        ))
        .transcript(&s.id),
    )
    .unwrap();
    assert!(
        raw.contains("\"ai-title\""),
        "expected ai-title entry in transcript: {raw}",
    );
    assert!(
        !raw.contains("\"custom-title\""),
        "apply_title must not write custom-title: {raw}",
    );
}

#[test]
fn apply_title_appends_ai_title_even_when_custom_exists() {
    // User has already renamed; apply_title still records the
    // AI-suggested title (lower-priority fallback) without
    // clobbering the user-set CustomTitle. Read precedence —
    // `read_transcript_metadata`'s `custom_title > ai_title` — keeps
    // the user title visible.
    let tmp = tempfile::tempdir().unwrap();
    let mgr = crate::SessionManager::new(tmp.path().to_path_buf());
    seed_transcript(tmp.path(), "sess-b");
    mgr.set_title("sess-b", "User-set title").unwrap();

    let applied = apply_title(&mgr, "sess-b", "Auto title".into()).unwrap();
    assert!(!applied, "session was already titled");

    let reloaded = mgr.load("sess-b").unwrap();
    assert_eq!(reloaded.title.as_deref(), Some("User-set title"));
}

#[test]
fn session_name_prompt_includes_conversation_body() {
    let (system, user) = build_session_name_prompt("User: fix the login crash");
    assert!(system.contains("kebab-case"));
    assert!(system.contains("\"name\""));
    assert!(user.contains("fix the login crash"));
}

#[test]
fn session_name_prompt_truncates_long_inputs_safely() {
    let mut text = "a".repeat(CONVERSATION_CONTEXT_CHARS - 2);
    text.push('é');
    text.push_str("bb");
    let (_system, user) = build_session_name_prompt(&text);
    assert_eq!(user, text);
}

#[test]
fn parse_session_name_strict_json_object() {
    assert_eq!(
        parse_session_name_response(r#"{"name":"fix-login-bug"}"#),
        Some("fix-login-bug".into())
    );
}

#[test]
fn parse_session_name_preserves_structured_name() {
    assert_eq!(
        parse_session_name_response(r#"{"name":"Fix Login Bug"}"#),
        Some("Fix Login Bug".into())
    );
}

#[test]
fn parse_session_name_rejects_empty_and_non_object_fallbacks() {
    assert_eq!(parse_session_name_response(""), None);
    assert_eq!(parse_session_name_response("  "), None);
    assert_eq!(parse_session_name_response(r#"{"name":""}"#), None);
    assert_eq!(parse_session_name_response("\"add-auth-feature\""), None);
    assert_eq!(parse_session_name_response("debug-test-failures"), None);
    assert_eq!(parse_session_name_response("name: add-caching"), None);
}

#[test]
fn parse_session_name_accepts_ts_valid_short_and_long_strings() {
    assert_eq!(
        parse_session_name_response(r#"{"name":"a"}"#),
        Some("a".into())
    );
    let long = "fix-".repeat(40);
    let raw = format!(r#"{{"name":"{long}"}}"#);
    let parsed = parse_session_name_response(&raw).unwrap();
    assert_eq!(parsed, long);
}

#[test]
fn parse_session_name_tool_input_reads_name_field() {
    assert_eq!(
        parse_session_name_tool_input(&serde_json::json!({"name": "fix-login-bug"})),
        Some("fix-login-bug".into())
    );
}

#[test]
fn extract_conversation_text_skips_prefixes_and_keeps_tail_after_compact() {
    let before = coco_messages::create_user_message("old topic");
    let boundary = coco_messages::create_compact_boundary_message(10_000, 2_000);
    let after = coco_messages::create_user_message("fix login crash");
    let assistant = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::text("patch auth handler")],
        "model",
        coco_types::TokenUsage::default(),
    );

    let text = extract_conversation_text(&[before, boundary, after, assistant]);

    assert_eq!(text, "fix login crash\npatch auth handler");
    assert!(!text.contains("User:"));
    assert!(!text.contains("Assistant:"));
}
