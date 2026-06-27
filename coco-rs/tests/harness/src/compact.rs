//! Compact test utilities: mock summarize_fn factories and assertion helpers.
//!
//! The mock functions return complex `impl Fn(CompactSummaryAttempt) -> Pin<Box<...>>` types
//! because that's what `compact_conversation`'s generic `F` bound requires.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use coco_compact::CompactResult;
use coco_compact::CompactSummaryAttempt;
use coco_compact::CompactSummaryResponse;
use coco_messages::Message;
use coco_types::CompactTrigger;

// ── Mock summarize_fn factories ─────────────────────────────────────

/// Returns a summarize_fn that always succeeds with the given summary.
pub fn mock_summarize_ok(
    summary: &str,
) -> impl Fn(
    CompactSummaryAttempt,
) -> Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
+ Send
+ Sync {
    let s = summary.to_string();
    move |_attempt| {
        let s = s.clone();
        Box::pin(async move { Ok(CompactSummaryResponse { summary: s }) })
    }
}

/// Returns a summarize_fn that captures attempts for assertion, plus the capture vec.
pub fn mock_summarize_capturing(
    summary: &str,
) -> (
    impl Fn(
        CompactSummaryAttempt,
    ) -> Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
    + Send
    + Sync,
    Arc<Mutex<Vec<CompactSummaryAttempt>>>,
) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    let s = summary.to_string();
    let f = move |attempt: CompactSummaryAttempt| {
        let cap = cap.clone();
        let s = s.clone();
        Box::pin(async move {
            cap.lock().expect("test mutex poisoned").push(attempt);
            Ok(CompactSummaryResponse { summary: s })
        }) as Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
    };
    (f, captured)
}

/// Returns a summarize_fn that fails N times with "prompt_too_long", then succeeds.
pub fn mock_summarize_ptl_then_ok(
    ptl_count: i32,
    summary: &str,
) -> impl Fn(
    CompactSummaryAttempt,
) -> Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
+ Send
+ Sync {
    let counter = Arc::new(AtomicI32::new(0));
    let s = summary.to_string();
    move |_attempt| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        let s = s.clone();
        Box::pin(async move {
            if n < ptl_count {
                Err("prompt_too_long: input exceeds context".to_string())
            } else {
                Ok(CompactSummaryResponse { summary: s })
            }
        })
    }
}

/// Returns a summarize_fn that fails N times with a transient error, then succeeds.
pub fn mock_summarize_fail_then_ok(
    fail_count: i32,
    summary: &str,
) -> impl Fn(
    CompactSummaryAttempt,
) -> Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
+ Send
+ Sync {
    let counter = Arc::new(AtomicI32::new(0));
    let s = summary.to_string();
    move |_attempt| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        let s = s.clone();
        Box::pin(async move {
            if n < fail_count {
                Err("transient stream error".to_string())
            } else {
                Ok(CompactSummaryResponse { summary: s })
            }
        })
    }
}

/// Returns a summarize_fn that always fails with the given error.
pub fn mock_summarize_always_fail(
    error: &str,
) -> impl Fn(
    CompactSummaryAttempt,
) -> Pin<Box<dyn Future<Output = Result<CompactSummaryResponse, String>> + Send>>
+ Send
+ Sync {
    let e = error.to_string();
    move |_attempt| {
        let e = e.clone();
        Box::pin(async move { Err(e) })
    }
}

// ── Assertion helpers ───────────────────────────────────────────────

/// Assert the CompactResult boundary_marker is a valid CompactBoundary.
pub fn assert_boundary_valid(result: &CompactResult) {
    let Message::System(coco_messages::SystemMessage::CompactBoundary(ref b)) =
        result.boundary_marker
    else {
        panic!(
            "boundary_marker should be CompactBoundary, got: {:?}",
            result.boundary_marker
        );
    };
    assert!(b.tokens_before >= 0, "tokens_before should be non-negative");
    assert!(b.tokens_after >= 0, "tokens_after should be non-negative");
}

/// Assert the CompactResult summary_messages contains a valid compact summary.
pub fn assert_summary_valid(result: &CompactResult) {
    assert!(
        !result.summary_messages.is_empty(),
        "summary_messages should not be empty"
    );
    let Message::User(ref u) = result.summary_messages[0] else {
        panic!(
            "summary_messages[0] should be User, got: {:?}",
            result.summary_messages[0]
        );
    };
    assert!(u.is_compact_summary, "should have is_compact_summary=true");
    assert!(
        u.is_visible_in_transcript_only,
        "should have is_visible_in_transcript_only=true"
    );
}

/// Build a minimal valid CompactResult for tests that need one as input.
pub fn dummy_compact_result() -> CompactResult {
    let boundary = Message::System(coco_messages::SystemMessage::CompactBoundary(
        coco_messages::SystemCompactBoundaryMessage {
            uuid: uuid::Uuid::new_v4(),
            tokens_before: 1000,
            tokens_after: 200,
            trigger: CompactTrigger::Auto,
            user_context: None,
            messages_summarized: Some(5),
            pre_compact_discovered_tools: vec![],
            preserved_segment: None,
        },
    ));
    CompactResult {
        boundary_marker: boundary,
        raw_summary: None,
        summary_messages: vec![],
        attachments: vec![],
        messages_to_keep: vec![],
        hook_results: vec![],
        user_display_message: None,
        pre_compact_tokens: 1000,
        post_compact_tokens: 200,
        true_post_compact_tokens: 200,
        is_recompaction: false,
        trigger: CompactTrigger::Auto,
    }
}
