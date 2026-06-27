//! Pure state-machine tests for [`SuspendContext`]. Cannot exercise the
//! real syscall path (`libc::kill(0, SIGTSTP)`) inside nextest — it
//! would stop the test binary.

#![cfg(unix)]

use super::*;

#[test]
fn take_resume_pending_is_false_on_a_fresh_context() {
    let ctx = SuspendContext::new();
    assert!(!ctx.take_resume_pending());
}

#[test]
fn take_resume_pending_consumes_the_pending_flag() {
    let ctx = SuspendContext::new();
    *ctx.resume_pending.lock().unwrap() = true;

    assert!(
        ctx.take_resume_pending(),
        "first take should yield the pending flag"
    );
    assert!(
        !ctx.take_resume_pending(),
        "the pending flag must be drained after one take"
    );
}
