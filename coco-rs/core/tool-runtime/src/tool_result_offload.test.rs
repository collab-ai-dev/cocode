use pretty_assertions::assert_eq;

use super::*;
use crate::tool_result_storage::ToolOutputStore;
use crate::tool_result_storage::tool_results_dir;

fn line_start(s: &str, byte: usize) -> bool {
    byte == 0 || s.as_bytes()[byte - 1] == b'\n'
}

/// Core coverage property: every byte of the source is either shown (head or
/// tail) or inside the reported omitted line range — no unreported gap. The
/// omitted range may OVERLAP shown content (conservative), never undershoot.
fn check_coverage(input: &str, budget: InlineBudget) {
    let wrapped = hard_wrap(input, HARD_WRAP_WIDTH);
    let Some(view) = WindowedView::compute(&wrapped, budget) else {
        return; // fits — nothing omitted
    };
    let head_end = view.head.len();
    let tail_start = wrapped.len() - view.tail.len();

    // Never overlap.
    assert!(head_end <= tail_start, "head/tail overlap");

    // omitted_start_line = count('\n' in head) + 1 in BOTH snap cases.
    assert_eq!(
        view.omitted_start_line,
        wrapped[..head_end].matches('\n').count() + 1,
        "omitted_start_line formula"
    );

    // Byte reconstruction: the three ranges tile the wrapped text exactly.
    assert_eq!(
        format!(
            "{}{}{}",
            view.head,
            &wrapped[head_end..tail_start],
            view.tail
        ),
        *wrapped,
        "byte reconstruction"
    );

    // Line-range coverage: the omitted middle's bytes all lie on lines within
    // [omitted_start_line, omitted_end_line].
    let full_lines_before_tail = wrapped[..tail_start].matches('\n').count();
    let expected_end = if line_start(&wrapped, tail_start) {
        full_lines_before_tail
    } else {
        // Tail begins mid-line: the partial line must be INCLUDED in the
        // omitted range so its unseen first half is recoverable.
        full_lines_before_tail + 1
    };
    assert_eq!(
        view.omitted_end_line, expected_end,
        "omitted_end_line coverage"
    );
    assert_eq!(
        view.tail_start_line,
        full_lines_before_tail + 1,
        "tail_start_line"
    );
}

#[test]
fn windowed_view_covers_across_content_classes() {
    let big_line = "x".repeat(500);
    let cases: Vec<String> = vec![
        (0..2000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
        // CJK at the cut points.
        "你好世界".repeat(4000),
        // Emoji.
        "🎉🎈".repeat(4000),
        // Box-drawing glyph (multi-byte) that has shipped panics.
        "─".repeat(8000),
        // Single very long line (forces wrapping).
        big_line.repeat(300),
        // CRLF content.
        (0..2000)
            .map(|i| format!("row {i}\r"))
            .collect::<Vec<_>>()
            .join("\n"),
    ];
    for case in &cases {
        check_coverage(case, REFERENCE_BUDGET);
        check_coverage(case, InlineBudget::from_request(30_000));
        // Floor-sized budget: tail sub-budget (500) is close to the wrap
        // width, so mid-line tail starts become reachable.
        check_coverage(case, InlineBudget::from_request(2_000));
    }
}

#[test]
fn tail_mid_line_start_is_counted_as_omitted() {
    // Budget 2_000 → tail budget 500, snap threshold 250. Lines of 520 bytes
    // (521 with '\n'): the raw tail cut lands 20 bytes into a line
    // (total−500 ≡ 20 mod 521), so advancing to the next line boundary costs
    // 500 ≥ 250 and the snap is skipped — the tail genuinely starts mid-line.
    // (Direct compute test — no hard_wrap, which would forbid 520-byte lines.)
    let line = "y".repeat(520);
    let content = (0..40).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
    let view = WindowedView::compute(&content, InlineBudget::from_request(2_000)).unwrap();
    let tail_start = content.len() - view.tail.len();
    assert!(
        !line_start(&content, tail_start),
        "fixture must exercise the mid-line tail case"
    );
    // The partial line is part of the omitted range (conservative — its first
    // half is unseen), and the tail starts ON that same line.
    let full_lines_before_tail = content[..tail_start].matches('\n').count();
    assert_eq!(view.omitted_end_line, full_lines_before_tail + 1);
    assert_eq!(view.tail_start_line, view.omitted_end_line);
}

#[test]
fn windowed_view_none_when_fits() {
    assert!(WindowedView::compute("small", REFERENCE_BUDGET).is_none());
    assert!(WindowedView::compute("", REFERENCE_BUDGET).is_none());
    // Content exactly at budget is not windowed.
    let exact = "a".repeat(REFERENCE_BUDGET.bytes());
    assert!(WindowedView::compute(&exact, REFERENCE_BUDGET).is_none());
    // One byte over is windowed.
    let over = "a".repeat(REFERENCE_BUDGET.bytes() + 1);
    assert!(WindowedView::compute(&over, REFERENCE_BUDGET).is_some());
}

#[test]
fn windowed_view_raw_single_line_no_panic() {
    // Direct compute on unwrapped, newline-free content: mid-line head,
    // degenerate line numbers, but never panics and never overlaps.
    let s = "a".repeat(300_000);
    let view = WindowedView::compute(&s, InlineBudget::from_request(10_000)).unwrap();
    assert!(view.head.len() + view.tail.len() <= s.len());
    assert_eq!(view.omitted_start_line, 1);
    // Single line, tail starts mid-line → the line itself is "omitted"
    // (conservative) and the tail starts on it.
    assert_eq!(view.omitted_end_line, 1);
    assert_eq!(view.tail_start_line, 1);
}

#[test]
fn hard_wrap_bounds_line_length_and_is_idempotent() {
    let input = "abcdef ".repeat(500); // ~3500 bytes, no newlines
    let wrapped = hard_wrap(&input, HARD_WRAP_WIDTH);
    assert!(wrapped.split('\n').all(|l| l.len() <= HARD_WRAP_WIDTH));
    // Idempotent: re-wrapping an already-wrapped string borrows unchanged.
    let again = hard_wrap(&wrapped, HARD_WRAP_WIDTH);
    assert_eq!(*again, *wrapped);
    assert!(matches!(again, std::borrow::Cow::Borrowed(_)));
}

#[test]
fn hard_wrap_preserves_short_lines() {
    let input = "short\nlines\nhere";
    assert!(matches!(
        hard_wrap(input, HARD_WRAP_WIDTH),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn hard_wrap_prefers_whitespace_break() {
    let width = 10;
    // "hello worldxx" -> break after "hello " (whitespace past half).
    let out = hard_wrap("hello worldxx", width);
    assert_eq!(out, "hello \nworldxx");
}

#[test]
fn scaled_per_message_bytes_table() {
    assert_eq!(scaled_per_message_bytes(32_768), 39_321);
    assert_eq!(scaled_per_message_bytes(65_536), 78_643);
    // >= 200k tokens clamps to the fixed default (byte-identical to today).
    assert_eq!(scaled_per_message_bytes(200_000), 200_000);
    assert_eq!(scaled_per_message_bytes(1_000_000), 200_000);
    // Unknown / zero window falls back to the fixed default.
    assert_eq!(scaled_per_message_bytes(0), 200_000);
    assert_eq!(scaled_per_message_bytes(-5), 200_000);
    // Tiny window still respects the floor.
    assert_eq!(scaled_per_message_bytes(1_000), 16_000);
}

#[test]
fn inline_budget_constructors() {
    // Config: reject non-positive, keep default.
    assert!(InlineBudget::try_new(0).is_none());
    assert!(InlineBudget::try_new(-1).is_none());
    assert_eq!(
        InlineBudget::try_new(4_000).map(InlineBudget::get),
        Some(4_000)
    );
    // Model param: clamp into range.
    assert_eq!(
        InlineBudget::from_request(1).get(),
        InlineBudget::MIN_REQUEST
    );
    assert_eq!(
        InlineBudget::from_request(9_999_999).get(),
        InlineBudget::MAX_REQUEST
    );
    assert_eq!(InlineBudget::from_request(20_000).get(), 20_000);
}

#[test]
fn inline_budget_capped_to_threshold() {
    // Large threshold: unchanged.
    assert_eq!(REFERENCE_BUDGET.capped_to(50_000).get(), 4_000);
    // Small threshold: capped to threshold - reserve, floored at MIN_REQUEST.
    assert_eq!(
        InlineBudget::from_request(30_000).capped_to(30_000).get(),
        29_000
    );
    assert_eq!(
        REFERENCE_BUDGET.capped_to(8).get(),
        InlineBudget::MIN_REQUEST
    );
}

#[test]
fn is_pointer_bearing_matches_prefix_and_suffix() {
    assert!(is_pointer_bearing(
        "<persisted-output>\n...\n</persisted-output>"
    ));
    assert!(is_pointer_bearing("head\n\ntail\n\n</persisted-output>"));
    assert!(!is_pointer_bearing("just plain output"));
}

#[tokio::test]
async fn offload_without_store_degrades_to_pointerless_window() {
    let content = "line\n".repeat(4_000); // > REFERENCE_BUDGET
    let key = ArtifactKey::ToolUse {
        id: "t1".into(),
        is_json: false,
    };
    let out = offload_windowed(None, &key, &content, REFERENCE_BUDGET).await;
    assert!(out.was_windowed);
    assert!(out.stored_path.is_none());
    assert!(out.model_text.contains("Full text not saved"));
    // Tag wraps only the trailing footer — text does NOT start with the tag.
    assert!(!out.model_text.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(
        out.model_text
            .trim_end()
            .ends_with(PERSISTED_OUTPUT_CLOSING_TAG)
    );
    // But it IS pointer-bearing (suffix footer).
    assert!(is_pointer_bearing(&out.model_text));
}

#[tokio::test]
async fn offload_with_store_writes_artifact_and_navigable_footer() {
    let dir = tempfile::tempdir().unwrap();
    let store = ToolOutputStore::new(dir.path());
    let content = (0..2_000)
        .map(|i| format!("row {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let key = ArtifactKey::ToolUse {
        id: "call-1".into(),
        is_json: false,
    };
    let out = offload_windowed(
        Some(&store),
        &key,
        &content,
        InlineBudget::from_request(4_000),
    )
    .await;

    assert!(out.was_windowed);
    let path = out.stored_path.expect("artifact written");
    assert!(path.exists());
    let footer = &out.model_text;
    assert!(footer.contains("limit=200"));
    assert!(footer.contains(&path.display().to_string()));

    // The suggested Read slice stays small: every artifact line is <= 400 bytes.
    let stored = std::fs::read_to_string(&path).unwrap();
    assert!(stored.split('\n').all(|l| l.len() <= HARD_WRAP_WIDTH));
}

#[tokio::test]
async fn named_key_atomic_publish_is_readable() {
    let dir = tempfile::tempdir().unwrap();
    let store = ToolOutputStore::new(dir.path());
    let content = "z".repeat(50_000);
    let key = ArtifactKey::Named {
        file_name: "url-example.com-abc123-def4.md".into(),
    };
    let out = offload_windowed(
        Some(&store),
        &key,
        &content,
        InlineBudget::from_request(10_000),
    )
    .await;
    let path = out.stored_path.expect("named artifact written");
    assert!(path.ends_with("url-example.com-abc123-def4.md"));
    // The artifact stores the HARD-WRAPPED text (Read-navigable): newlines are
    // inserted, but every original 'z' survives and lines stay <= 400 bytes.
    let stored = std::fs::read_to_string(&path).unwrap();
    assert_eq!(stored.bytes().filter(|b| *b == b'z').count(), content.len());
    assert!(stored.split('\n').all(|l| l.len() <= HARD_WRAP_WIDTH));
    let leftover: Vec<_> = std::fs::read_dir(tool_results_dir(dir.path()))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(leftover.is_empty(), "no tmp files left behind");
}

// ---------------------------------------------------------------------------
// Level 2 — per-message aggregate budget
// ---------------------------------------------------------------------------

use std::sync::Arc;
use tokio::sync::RwLock;

fn bash_candidate(id: &str, content: String) -> ToolResultCandidate {
    ToolResultCandidate {
        tool_use_id: id.into(),
        content_bytes: content.len() as i64,
        content,
        tool_name: Some("Bash".into()),
        persistence_opted_out: false,
        is_json: false,
    }
}

#[tokio::test]
async fn budget_inert_when_disabled() {
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(i64::MAX)));
    let candidates = vec![bash_candidate("id1", "x".repeat(1_000_000))];
    let tmp = tempfile::TempDir::new().unwrap();
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(outcome.newly_replaced.is_empty());
    assert_eq!(outcome.freed_bytes, 0);
    assert!(state.read().await.replacements.is_empty());
}

#[tokio::test]
async fn budget_windows_until_under_cap() {
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(15_000)));
    let tmp = tempfile::TempDir::new().unwrap();
    // Three 10K candidates = 30K; cap = 15K. Each windowed replacement (~4K)
    // frees ~6K, so the pass must window candidates until the aggregate fits.
    let candidates: Vec<ToolResultCandidate> = ["id1", "id2", "id3"]
        .iter()
        .map(|id| bash_candidate(id, "a".repeat(10_000)))
        .collect();
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(!outcome.newly_replaced.is_empty());

    // Convergence: aggregate after applying replacements is under the cap.
    let s = state.read().await;
    let aggregate: i64 = candidates
        .iter()
        .map(|c| match s.replacements.get(&c.tool_use_id) {
            Some(r) => r.len() as i64,
            None => c.content_bytes,
        })
        .sum();
    assert!(
        aggregate <= 15_000,
        "aggregate {aggregate} must fit the cap"
    );
    // All three are marked seen regardless of replacement.
    for id in ["id1", "id2", "id3"] {
        assert!(s.seen_ids.contains(id));
    }
    // Replacements are windowed (suffix footer), so the prefix check stays
    // false while the pointer-bearing check is true.
    for r in &outcome.newly_replaced {
        assert!(!is_content_already_persisted(&r.replacement));
        assert!(is_pointer_bearing(&r.replacement));
    }
}

#[tokio::test]
async fn budget_never_reoffloads_pointer_bearing_results() {
    // A Level-1-windowed result (suffix footer) counts toward the trigger but
    // must never be re-offloaded: under the same ToolUse id, create_new keeps
    // the existing full-text artifact while a re-render would compute footer
    // numbers from the windowed text — a pointer describing the wrong bytes.
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(15_000)));
    let tmp = tempfile::TempDir::new().unwrap();

    // Simulate a Level-1 windowed result: window a 30K output first.
    let store = ToolOutputStore::new(tmp.path());
    let key = ArtifactKey::ToolUse {
        id: "windowed".into(),
        is_json: false,
    };
    let full = "line one two three\n".repeat(1_600); // ~30K
    let windowed = offload_windowed(Some(&store), &key, &full, REFERENCE_BUDGET).await;
    assert!(windowed.was_windowed);
    let artifact_before =
        std::fs::read_to_string(tmp.path().join("tool-results/windowed.txt")).unwrap();

    let candidates = vec![
        ToolResultCandidate {
            tool_use_id: "windowed".into(),
            content_bytes: windowed.model_text.len() as i64,
            content: windowed.model_text.clone(),
            tool_name: Some("Bash".into()),
            persistence_opted_out: false,
            is_json: false,
        },
        bash_candidate("fresh", "b".repeat(20_000)),
    ];
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;

    // Only the fresh plain candidate may be replaced.
    assert!(
        outcome
            .newly_replaced
            .iter()
            .all(|r| r.tool_use_id != "windowed"),
        "pointer-bearing result must never be re-offloaded"
    );
    assert!(
        outcome
            .newly_replaced
            .iter()
            .any(|r| r.tool_use_id == "fresh"),
        "the windowed result still counts toward the trigger total"
    );
    // The existing artifact is untouched.
    let artifact_after =
        std::fs::read_to_string(tmp.path().join("tool-results/windowed.txt")).unwrap();
    assert_eq!(artifact_before, artifact_after);
}

#[tokio::test]
async fn budget_skips_opted_out() {
    let state: ContentReplacementStateRef = Arc::new(RwLock::new(ContentReplacementState::new(50)));
    let mut read_candidate = bash_candidate("id1", "a".repeat(100));
    read_candidate.tool_name = Some("Read".into());
    read_candidate.persistence_opted_out = true;
    let candidates = vec![read_candidate, bash_candidate("id2", "b".repeat(100))];
    let tmp = tempfile::TempDir::new().unwrap();
    {
        let mut s = state.write().await;
        s.seen_ids.insert("id2".into());
    }
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(outcome.newly_replaced.is_empty());
}

#[tokio::test]
async fn budget_excludes_opted_out_from_trigger() {
    let state: ContentReplacementStateRef =
        Arc::new(RwLock::new(ContentReplacementState::new(15_000)));
    let tmp = tempfile::TempDir::new().unwrap();
    let mut read_candidate = bash_candidate("read", "a".repeat(20_000));
    read_candidate.tool_name = Some("Read".into());
    read_candidate.persistence_opted_out = true;
    let candidates = vec![read_candidate, bash_candidate("bash", "b".repeat(10_000))];
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
async fn budget_excludes_already_replaced_from_trigger() {
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
        bash_candidate("old", "x".repeat(20_000)),
        bash_candidate("new", "y".repeat(10_000)),
    ];
    let outcome = apply_tool_result_budget(&candidates, &state, tmp.path()).await;
    assert!(
        outcome.newly_replaced.is_empty(),
        "already-replaced 'old' is excluded; fresh 'new' (10K) is under the 15K cap"
    );
}
