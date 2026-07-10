use std::sync::Arc;
use std::sync::Mutex;

use coco_types::SessionId;
use pretty_assertions::assert_eq;

use super::SessionSeqAllocator;
use super::WATERMARK_PERSIST_INTERVAL;

fn session(raw: &str) -> SessionId {
    SessionId::try_new(raw).expect("valid test session id")
}

#[test]
fn next_is_strictly_monotonic_per_session_across_callers() {
    let allocator = Arc::new(SessionSeqAllocator::new());
    let a = session("seq-session-a");

    // Two "forwarders" drawing from the shared allocator interleave into one
    // strictly monotonic stream.
    let first = allocator.next(&a);
    let second = allocator.next(&a);
    let from_other_caller = Arc::clone(&allocator).next(&a);
    assert_eq!((first, second, from_other_caller), (1, 2, 3));
}

#[test]
fn sessions_get_independent_counters() {
    let allocator = SessionSeqAllocator::new();
    let a = session("seq-independent-a");
    let b = session("seq-independent-b");

    assert_eq!(allocator.next(&a), 1);
    assert_eq!(allocator.next(&a), 2);
    assert_eq!(allocator.next(&b), 1);
    assert_eq!(allocator.next(&a), 3);
    assert_eq!(allocator.next(&b), 2);
}

#[test]
fn initialize_after_watermark_skips_ahead_and_never_regresses() {
    let allocator = SessionSeqAllocator::new();
    allocator.set_skip_ahead_window(/*event_retention_per_session*/ 8);
    let a = session("seq-skip-ahead");

    allocator.initialize_after_watermark(&a, 100);
    let window = 8_i64.max(WATERMARK_PERSIST_INTERVAL) + WATERMARK_PERSIST_INTERVAL;
    assert_eq!(allocator.next(&a), 100 + window + 1);

    // A stale watermark observed later never moves the counter backwards.
    allocator.initialize_after_watermark(&a, 5);
    assert_eq!(allocator.next(&a), 100 + window + 2);
}

#[test]
fn persist_hook_fires_on_first_allocation_and_at_interval() {
    let allocator = SessionSeqAllocator::new();
    let persisted: Arc<Mutex<Vec<(SessionId, i64)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&persisted);
    allocator.set_persist_hook(Arc::new(move |session_id, seq| {
        sink.lock()
            .expect("persist sink lock")
            .push((session_id.clone(), seq));
    }));

    let a = session("seq-persist-hook");
    for _ in 0..(WATERMARK_PERSIST_INTERVAL * 2) {
        allocator.next(&a);
    }

    let seen = persisted.lock().expect("persist sink lock").clone();
    let seqs: Vec<i64> = seen.iter().map(|(_, seq)| *seq).collect();
    assert_eq!(seqs, vec![1, 1 + WATERMARK_PERSIST_INTERVAL]);
    assert!(seen.iter().all(|(id, _)| *id == a));
}

#[test]
fn resume_forces_prompt_watermark_persist() {
    let allocator = SessionSeqAllocator::new();
    allocator.set_skip_ahead_window(4);
    let persisted: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&persisted);
    allocator.set_persist_hook(Arc::new(move |_, seq| {
        sink.lock().expect("persist sink lock").push(seq);
    }));

    let a = session("seq-resume-persist");
    allocator.initialize_after_watermark(&a, 40);
    let first = allocator.next(&a);
    assert_eq!(
        persisted.lock().expect("persist sink lock").as_slice(),
        [first]
    );
}
