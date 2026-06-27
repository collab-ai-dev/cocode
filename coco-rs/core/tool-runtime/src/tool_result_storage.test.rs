use super::*;
use std::sync::Arc;
use tokio::sync::RwLock;

#[test]
fn test_resolve_persistence_threshold_opt_out() {
    assert_eq!(
        resolve_persistence_threshold(ResultSizeBound::Unbounded),
        ResultSizeBound::Unbounded,
    );
}

#[test]
fn test_resolve_persistence_threshold_clamps_to_default() {
    assert_eq!(
        resolve_persistence_threshold(ResultSizeBound::Chars(100_000)),
        ResultSizeBound::Chars(DEFAULT_MAX_RESULT_SIZE_CHARS),
    );
    assert_eq!(
        resolve_persistence_threshold(ResultSizeBound::Chars(10_000)),
        ResultSizeBound::Chars(10_000),
    );
}

#[tokio::test]
async fn test_persist_to_disk_writes_file_and_preview() {
    let tmp = tempfile::TempDir::new().unwrap();
    let big = format!("{}\n{}", "x".repeat(1_900), "y".repeat(6_000));
    let result = persist_to_disk(tmp.path(), "abc", &big, false)
        .await
        .unwrap();
    assert_eq!(result.original_size, big.len() as i64);
    assert!(result.has_more);
    assert!(result.preview.len() <= PREVIEW_SIZE_BYTES);
    assert!(result.preview.ends_with('\n'));
    assert_eq!(result.filepath, tool_result_path(tmp.path(), "abc", false));
    let on_disk = std::fs::read_to_string(&result.filepath).unwrap();
    assert_eq!(on_disk, big);
}

#[tokio::test]
async fn test_persist_to_disk_is_idempotent_existing_file_wins() {
    let tmp = tempfile::TempDir::new().unwrap();
    let first = "first".repeat(1_000);
    let second = "second".repeat(1_000);
    persist_to_disk(tmp.path(), "abc", &first, false)
        .await
        .unwrap();
    let result = persist_to_disk(tmp.path(), "abc", &second, false)
        .await
        .unwrap();
    assert_eq!(result.original_size, first.len() as i64);
    assert_eq!(
        std::fs::read_to_string(tool_result_path(tmp.path(), "abc", false)).unwrap(),
        first
    );
}

#[tokio::test]
async fn test_persist_mcp_binary_to_disk_uses_mime_extension_and_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let first = b"\x89PNG\r\n\x1a\nfirst";
    let second = b"second";
    let result = persist_mcp_binary_to_disk(tmp.path(), "mcp-1", first, Some("image/png"))
        .await
        .unwrap();
    assert_eq!(
        result.filepath,
        tool_results_dir(tmp.path()).join("mcp-1.png")
    );
    assert_eq!(result.original_size, first.len() as i64);
    assert_eq!(std::fs::read(&result.filepath).unwrap(), first);

    let second_result = persist_mcp_binary_to_disk(
        tmp.path(),
        "mcp-1",
        second,
        Some("image/png; charset=binary"),
    )
    .await
    .unwrap();
    assert_eq!(second_result.original_size, first.len() as i64);
    assert_eq!(std::fs::read(&result.filepath).unwrap(), first);
}

#[test]
fn test_generate_preview_respects_utf8_boundary() {
    let content = format!("{}é{}", "a".repeat(PREVIEW_SIZE_BYTES - 1), "b");
    let (preview, has_more) = generate_preview(&content, PREVIEW_SIZE_BYTES);
    assert!(has_more);
    assert!(preview.is_char_boundary(preview.len()));
    assert!(preview.len() <= PREVIEW_SIZE_BYTES);
}

#[tokio::test]
async fn test_apply_tool_result_budget_inert_when_disabled() {
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(i64::MAX)));
    let candidates = vec![ToolResultCandidate {
        tool_use_id: "id1".into(),
        content: "x".repeat(1_000_000),
        content_chars: 1_000_000,
        tool_name: Some("Bash".into()),
        persistence_opted_out: false,
        is_json: false,
    }];
    let tmp = tempfile::TempDir::new().unwrap();
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(outcome.newly_replaced.is_empty());
    assert_eq!(outcome.freed_chars, 0);
    assert!(state.read().await.replacements.is_empty());
}

#[tokio::test]
async fn test_apply_tool_result_budget_evicts_oldest_until_under_cap() {
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(100)));
    let tmp = tempfile::TempDir::new().unwrap();
    // Three candidates, total 30K chars. Budget = 15K. Level 2 picks
    // the largest fresh candidates. Equal sizes preserve input order.
    let candidates = vec![
        ToolResultCandidate {
            tool_use_id: "id1".into(),
            content: "a".repeat(10_000),
            content_chars: 10_000,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
        ToolResultCandidate {
            tool_use_id: "id2".into(),
            content: "b".repeat(10_000),
            content_chars: 10_000,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
        ToolResultCandidate {
            tool_use_id: "id3".into(),
            content: "c".repeat(10_000),
            content_chars: 10_000,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
    ];
    {
        let mut s = state.write().await;
        s.per_message_chars = 15_000;
    }
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert_eq!(
        outcome
            .newly_replaced
            .iter()
            .map(|r| r.tool_use_id.as_str())
            .collect::<Vec<_>>(),
        vec!["id1", "id2"]
    );
    let s = state.read().await;
    assert!(s.replacements.contains_key("id1"));
    assert!(s.replacements.contains_key("id2"));
    assert!(!s.replacements.contains_key("id3"));
    // All three must be marked seen.
    assert!(s.seen_ids.contains("id1"));
    assert!(s.seen_ids.contains("id2"));
    assert!(s.seen_ids.contains("id3"));
}

#[tokio::test]
async fn test_apply_tool_result_budget_skips_opted_out() {
    let state: ContentReplacementStateRef = Arc::new(RwLock::new(ContentReplacementState::new(50)));
    let candidates = vec![
        ToolResultCandidate {
            tool_use_id: "id1".into(),
            content: "a".repeat(100),
            content_chars: 100,
            tool_name: Some("Read".into()),
            persistence_opted_out: true, // Read on canonical file — opt out
            is_json: false,
        },
        ToolResultCandidate {
            tool_use_id: "id2".into(),
            content: "b".repeat(100),
            content_chars: 100,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
    ];
    let tmp = tempfile::TempDir::new().unwrap();
    {
        let mut s = state.write().await;
        s.seen_ids.insert("id2".into());
    }
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    // id1 is over cap but opted out; id2 was already seen and is frozen.
    assert!(outcome.newly_replaced.is_empty());
}

#[tokio::test]
async fn test_apply_tool_result_budget_excludes_opted_out_from_trigger() {
    // The trigger total must count only eligible candidates. A large opted-out
    // Read (20K) plus a small fresh Bash (10K) is 30K of raw content, but only
    // the 10K Bash is eligible — under the 15K cap — so nothing persists.
    // (Old behavior counted the opted-out 20K and would have evicted the Bash.)
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(15_000)));
    let tmp = tempfile::TempDir::new().unwrap();
    let candidates = vec![
        ToolResultCandidate {
            tool_use_id: "read".into(),
            content: "a".repeat(20_000),
            content_chars: 20_000,
            tool_name: Some("Read".into()),
            persistence_opted_out: true,
            is_json: false,
        },
        ToolResultCandidate {
            tool_use_id: "bash".into(),
            content: "b".repeat(10_000),
            content_chars: 10_000,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
    ];
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(
        outcome.newly_replaced.is_empty(),
        "eligible total (10K Bash) is under the 15K cap; opted-out Read must not count"
    );
    let s = state.read().await;
    assert!(s.seen_ids.contains("read"));
    assert!(s.seen_ids.contains("bash"));
}

#[tokio::test]
async fn test_apply_tool_result_budget_excludes_already_replaced_from_trigger() {
    // An already-replaced id contributes 0 to the trigger (mustReapply), so a
    // lone fresh candidate under the cap persists nothing even though the raw
    // sum of both contents exceeds the budget.
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(15_000)));
    let tmp = tempfile::TempDir::new().unwrap();
    {
        let mut s = state.write().await;
        s.seen_ids.insert("old".into());
        s.replacements.insert(
            "old".into(),
            "<persisted-output>…</persisted-output>".into(),
        );
    }
    let candidates = vec![
        ToolResultCandidate {
            tool_use_id: "old".into(),
            content: "x".repeat(20_000),
            content_chars: 20_000,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
        ToolResultCandidate {
            tool_use_id: "new".into(),
            content: "y".repeat(10_000),
            content_chars: 10_000,
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
    ];
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(
        outcome.newly_replaced.is_empty(),
        "already-replaced 'old' is excluded; fresh 'new' (10K) is under the 15K cap"
    );
}

#[test]
fn test_render_persisted_reference_includes_filepath_and_preview() {
    let p = PersistedToolResult {
        filepath: PathBuf::from("/sess/tool-results/abc.txt"),
        original_size: 12_345,
        is_json: false,
        preview: "first chars...".into(),
        has_more: true,
    };
    let rendered = render_persisted_reference(&p);
    assert!(rendered.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(rendered.ends_with(PERSISTED_OUTPUT_CLOSING_TAG));
    assert!(rendered.contains("/sess/tool-results/abc.txt"));
    assert!(rendered.contains("12.1KB"));
    assert!(rendered.contains("first chars..."));
}

#[test]
fn test_render_mcp_binary_reference_includes_filepath_and_mime() {
    let p = PersistedMcpBinaryOutput {
        filepath: PathBuf::from("/sess/tool-results/mcp-1.pdf"),
        original_size: 4_096,
        mime_type: "application/pdf".into(),
    };
    let rendered = render_mcp_binary_reference(&p);
    assert!(rendered.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(rendered.ends_with(PERSISTED_OUTPUT_CLOSING_TAG));
    assert!(rendered.contains("MCP output is binary"));
    assert!(rendered.contains("4KB"));
    assert!(rendered.contains("application/pdf"));
    assert!(rendered.contains("/sess/tool-results/mcp-1.pdf"));
}
