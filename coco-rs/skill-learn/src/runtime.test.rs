use super::{ReviewTrigger, SkillReviewRuntime};
use coco_types::SessionId;

fn runtime(throttle: i32) -> (SkillReviewRuntime, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let rt = SkillReviewRuntime::with_throttle(tmp.path(), throttle);
    (rt, tmp)
}

fn session_id() -> SessionId {
    SessionId::try_new("s").unwrap()
}

#[tokio::test]
async fn subagent_turns_are_skipped_and_do_not_count() {
    let (rt, _tmp) = runtime(2);
    assert_eq!(
        rt.maybe_review(
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
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Throttled
    );
    assert_eq!(
        rt.maybe_review(
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
            /*turn_delivered*/ false,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Skipped
    );
}

#[tokio::test]
async fn fires_every_throttle_turns() {
    let (rt, _tmp) = runtime(3);
    assert_eq!(
        rt.maybe_review(
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Throttled
    );
    assert_eq!(
        rt.maybe_review(
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Throttled
    );
    assert_eq!(
        rt.maybe_review(
            /*turn_delivered*/ true,
            /*is_subagent*/ false,
            &session_id(),
            Vec::new,
        ),
        ReviewTrigger::Spawned
    );
}

#[tokio::test]
async fn in_progress_suppresses_then_retries_on_next_eligible_turn() {
    use std::sync::atomic::Ordering;

    let (rt, _tmp) = runtime(1);
    rt.in_progress.store(true, Ordering::SeqCst);
    assert_eq!(
        rt.maybe_review(
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
