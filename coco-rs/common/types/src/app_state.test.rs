use super::ElicitationGuard;
use super::PendingPermissionGuard;
use super::ToolAppState;
use crate::PermissionMode;
use std::sync::atomic::Ordering;

#[test]
fn default_is_all_zero() {
    // Sanity: a fresh session has no plan-mode latches, no counters, no
    // pending approval. Drivers rely on Default() matching the "empty
    // JSON object" behavior the old serde_json representation had.
    let s = ToolAppState::default();
    assert_eq!(s.permissions.mode, None);
    assert_eq!(s.permissions.pre_plan_mode, None);
    assert!(s.permissions.stripped_dangerous_rules.is_none());
    assert!(!s.has_exited_plan_mode);
    assert!(!s.needs_plan_mode_exit_attachment);
    assert_eq!(s.pending_plan_mode_exit_outcome, None);
    assert_eq!(s.last_permission_mode, None);
    assert_eq!(s.plan_mode_entry_ms, None);
    assert!(!s.awaiting_plan_approval);
    assert_eq!(s.awaiting_plan_approval_request_id, None);
    // Phase 7 wire-up: counters start at 0, rate_limits is empty.
    assert_eq!(s.pending_permission_count.load(Ordering::Relaxed), 0);
    assert_eq!(s.elicitation_pending_count.load(Ordering::Relaxed), 0);
    assert!(s.rate_limits.is_empty());
}

#[test]
fn pending_permission_guard_increments_and_drops() {
    // Lock-free counter behaviour: acquire bumps the atomic, drop
    // decrements. This is the contract the prompt-suggestion fork
    // relies on — counter > 0 ↔ at least one overlay is open.
    let s = ToolAppState::default();
    let counter = &s.pending_permission_count;
    assert_eq!(counter.load(Ordering::Relaxed), 0);

    let g1 = PendingPermissionGuard::acquire(std::sync::Arc::clone(counter));
    assert_eq!(counter.load(Ordering::Relaxed), 1);

    let g2 = PendingPermissionGuard::acquire(std::sync::Arc::clone(counter));
    assert_eq!(counter.load(Ordering::Relaxed), 2);

    drop(g1);
    assert_eq!(counter.load(Ordering::Relaxed), 1);

    drop(g2);
    assert_eq!(counter.load(Ordering::Relaxed), 0);
}

#[test]
fn elicitation_guard_increments_and_drops() {
    // Same shape as PendingPermissionGuard, pinned to the
    // elicitation counter. Verify it works independently — both
    // counters live on the same struct but don't cross-talk.
    let s = ToolAppState::default();
    let perm_counter = &s.pending_permission_count;
    let elicit_counter = &s.elicitation_pending_count;

    let _g = ElicitationGuard::acquire(std::sync::Arc::clone(elicit_counter));
    assert_eq!(elicit_counter.load(Ordering::Relaxed), 1);
    // Permission counter must NOT move — counters are independent.
    assert_eq!(perm_counter.load(Ordering::Relaxed), 0);
}

#[test]
fn pending_permission_guard_drop_in_panic_unwind() {
    // Drop is sync + lock-free, so it fires correctly even on
    // panic-unwind. This is the property that lets us use the
    // guard from arbitrary tasks without runtime concerns.
    let s = ToolAppState::default();
    let counter = &s.pending_permission_count;

    let counter_for_closure = std::sync::Arc::clone(counter);
    let _ = std::panic::catch_unwind(move || {
        let _guard = PendingPermissionGuard::acquire(counter_for_closure);
        panic!("simulate task panic with guard held");
    });

    assert_eq!(
        counter.load(Ordering::Relaxed),
        0,
        "panic unwind must still drop the guard and decrement"
    );
}

#[test]
fn struct_update_syntax_composes() {
    // Tests previously built ad-hoc snapshots with `json!({...})`.
    // The struct-update spread is the replacement idiom — verify it
    // produces the expected field values so migrating tests is trivial.
    let s = ToolAppState {
        awaiting_plan_approval: true,
        awaiting_plan_approval_request_id: Some("plan_approval-alice-a-1".into()),
        last_permission_mode: Some(PermissionMode::AcceptEdits),
        ..Default::default()
    };
    assert!(s.awaiting_plan_approval);
    assert_eq!(
        s.awaiting_plan_approval_request_id.as_deref(),
        Some("plan_approval-alice-a-1")
    );
    assert_eq!(s.last_permission_mode, Some(PermissionMode::AcceptEdits));
    // Unfilled fields stay default.
    assert!(!s.has_exited_plan_mode);
}
