use super::{ReviewSignal, ReviewTrigger, SkillReviewRuntime};
use coco_types::SessionId;

fn runtime(throttle: i32) -> (SkillReviewRuntime, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let rt = SkillReviewRuntime::with_throttle(tmp.path(), throttle);
    (rt, tmp)
}

fn session_id() -> SessionId {
    SessionId::try_new("s").unwrap()
}

/// A signal with enough material work to clear the default 3-tool-call gate.
fn material() -> ReviewSignal {
    ReviewSignal {
        tool_calls: 3,
        skill_invoked: false,
    }
}

#[tokio::test]
async fn subagent_turns_are_skipped_and_do_not_count() {
    let (rt, _tmp) = runtime(2);
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ true,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Skipped
    );
    // A subagent turn must not advance the throttle counter: the next two
    // non-subagent turns are still Throttled then Spawned.
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Throttled
    );
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned
    );
}

#[tokio::test]
async fn undelivered_turns_are_skipped() {
    let (rt, _tmp) = runtime(1);
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ false,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Skipped
    );
}

#[tokio::test]
async fn empty_signal_skips_at_zero_cost_without_advancing_throttle() {
    let (rt, _tmp) = runtime(1);
    let empty = ReviewSignal::default();
    assert_eq!(
        rt.maybe_review(
            empty,
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Skipped,
        "no material work → no fork even at the throttle boundary"
    );
    // The counter never advanced, so a material turn still fires immediately.
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned
    );
}

#[tokio::test]
async fn skill_invocation_alone_is_material_signal() {
    let (rt, _tmp) = runtime(1);
    let signal = ReviewSignal {
        tool_calls: 0,
        skill_invoked: true,
    };
    assert_eq!(
        rt.maybe_review(
            signal,
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned,
        "an invoked skill is signal even with no tool calls"
    );
}

#[tokio::test]
async fn fires_every_throttle_turns() {
    let (rt, _tmp) = runtime(3);
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Throttled
    );
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Throttled
    );
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned
    );
}

#[tokio::test]
async fn failure_backoff_stretches_the_effective_throttle() {
    use std::sync::atomic::Ordering;

    let (rt, _tmp) = runtime(1);
    // Two consecutive failures → effective throttle 1 << 2 == 4.
    rt.consecutive_failures.store(2, Ordering::SeqCst);
    assert_eq!(rt.effective_throttle(), 4);
    for _ in 0..3 {
        assert_eq!(
            rt.maybe_review(
                material(),
                /*turn_delivered*/ true,
                /*is_subagent*/ false,
                &session_id(),
                Vec::new,
            ),
            ReviewTrigger::Throttled,
            "a failing spawn stretches the next fire"
        );
    }
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned
    );
}

#[tokio::test]
async fn backoff_shift_is_capped() {
    use std::sync::atomic::Ordering;

    let (rt, _tmp) = runtime(2);
    rt.consecutive_failures.store(99, Ordering::SeqCst);
    // Clamped to a 5-bit shift: 2 << 5 == 64, not an unbounded value.
    assert_eq!(rt.effective_throttle(), 64);
}

#[tokio::test]
async fn manual_review_bypasses_throttle_but_respects_single_flight() {
    use std::sync::atomic::Ordering;

    let (rt, _tmp) = runtime(100);
    // Throttle is 100, yet a user-initiated /learn fires immediately.
    assert_eq!(
        rt.manual_review("learn the nextest filter".into(), &session_id(), Vec::new()),
        ReviewTrigger::Spawned
    );
    // While a review is in flight, a second manual request is suppressed.
    rt.in_progress.store(true, Ordering::SeqCst);
    assert_eq!(
        rt.manual_review("again".into(), &session_id(), Vec::new()),
        ReviewTrigger::InProgress
    );
}

#[tokio::test]
async fn in_progress_suppresses_then_retries_on_next_eligible_turn() {
    use std::sync::atomic::Ordering;

    let (rt, _tmp) = runtime(1);
    rt.in_progress.store(true, Ordering::SeqCst);
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::InProgress
    );
    // Once the prior fork finishes, the elevated counter fires immediately.
    rt.in_progress.store(false, Ordering::SeqCst);
    assert_eq!(
        rt.maybe_review(
            material(),
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned
    );
}

#[tokio::test]
async fn single_flight_flag_clears_even_when_the_detached_task_panics() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let flag = Arc::new(AtomicBool::new(true));
    let task_flag = flag.clone();
    let handle = tokio::spawn(async move {
        let _clear = super::ClearOnDrop(task_flag);
        panic!("simulated review panic");
    });
    assert!(handle.await.is_err(), "task must have panicked");
    assert!(
        !flag.load(Ordering::SeqCst),
        "a panicking fork must not wedge single-flight for the session"
    );
}
