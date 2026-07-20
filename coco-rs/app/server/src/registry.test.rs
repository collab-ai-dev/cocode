use super::*;

fn test_session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid test session id")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestHandle(&'static str);

#[test]
fn load_slot_promotes_to_live_and_unblocks_waiters() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-1");
    assert!(matches!(
        registry
            .begin_load(session_id.clone())
            .expect("reserve load"),
        LoadStart::Reserved
    ));
    let LoadStart::Loading(waiter) = registry
        .begin_load(session_id.clone())
        .expect("observe loading")
    else {
        panic!("expected loading");
    };

    registry
        .complete_load_success(&session_id, TestHandle("h1"))
        .expect("complete load");

    assert_eq!(
        waiter.ready().expect("load ready").expect("load ok"),
        TestHandle("h1")
    );
    assert_eq!(registry.get(&session_id), Some(TestHandle("h1")));
    assert_eq!(registry.live_count(), 1);
}

#[test]
fn load_failure_removes_slot_and_unblocks_waiters() {
    let registry = LiveSessionRegistry::<TestHandle>::new(4);
    let session_id = test_session_id("sess-1");
    registry
        .begin_load(session_id.clone())
        .expect("reserve load");
    let LoadStart::Loading(waiter) = registry
        .begin_load(session_id.clone())
        .expect("observe loading")
    else {
        panic!("expected loading");
    };
    let error = NotFoundSnafu {
        session_id: session_id.clone(),
    }
    .build();

    registry
        .complete_load_failure(&session_id, error.clone())
        .expect("complete failure");

    let Err(ready_error) = waiter.ready().expect("load ready") else {
        panic!("expected load error");
    };
    assert!(matches!(ready_error, RegistryError::NotFound { .. }));
    assert_eq!(ready_error.status_code(), error.status_code());
    assert_eq!(registry.slot_count(), 0);
}

#[test]
fn max_sessions_counts_loading_live_and_closing_slots() {
    let registry = LiveSessionRegistry::new(2);
    let loading = test_session_id("sess-loading");
    let live = test_session_id("sess-live");
    let blocked = test_session_id("sess-blocked");
    registry.begin_load(loading).expect("reserve loading slot");
    registry
        .begin_load(live.clone())
        .expect("reserve live slot");
    registry
        .complete_load_success(&live, TestHandle("live"))
        .expect("complete live");

    let err = registry
        .begin_load(blocked)
        .expect_err("registry should be full");

    assert!(matches!(err, RegistryError::ResourceExhausted { .. }));
    assert_eq!(err.status_code(), StatusCode::ResourcesExhausted);
    assert_eq!(registry.slot_count(), 2);
}

#[test]
fn begin_close_moves_live_slot_to_closing_until_completion() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-1");
    registry
        .begin_load(session_id.clone())
        .expect("reserve load");
    registry
        .complete_load_success(&session_id, TestHandle("h1"))
        .expect("complete load");

    let CloseStart::Started { handle, completion } =
        registry.begin_close(&session_id).expect("begin close")
    else {
        panic!("expected started close");
    };

    assert_eq!(handle, TestHandle("h1"));
    assert!(!completion.is_complete());
    assert_eq!(registry.get(&session_id), None);
    assert_eq!(registry.slot_count(), 1);
    let LoadStart::Closing(other_completion) = registry
        .begin_load(session_id.clone())
        .expect("observe closing")
    else {
        panic!("expected closing");
    };
    assert!(!other_completion.is_complete());
    let CloseStart::Closing {
        handle: closing_handle,
        completion: repeated_close,
    } = registry.begin_close(&session_id).expect("observe close")
    else {
        panic!("expected repeated closing");
    };
    assert_eq!(closing_handle, TestHandle("h1"));
    assert!(!repeated_close.is_complete());

    registry
        .complete_close(&session_id)
        .expect("complete close");

    assert!(completion.is_complete());
    assert_eq!(registry.slot_count(), 0);
}

#[test]
fn complete_close_with_result_propagates_close_error() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-1");
    registry
        .begin_load(session_id.clone())
        .expect("reserve load");
    registry
        .complete_load_success(&session_id, TestHandle("h1"))
        .expect("complete load");
    let CloseStart::Started { completion, .. } =
        registry.begin_close(&session_id).expect("begin close")
    else {
        panic!("expected started close");
    };
    let error = RegistryError::close_failed_with_data(
        "close timed out",
        Some(serde_json::json!({ "kind": "session_close_timeout" })),
    );

    registry
        .complete_close_with_result(&session_id, Err(error))
        .expect("complete close");

    let Err(ready_error) = completion.ready().expect("close ready") else {
        panic!("expected close error");
    };
    assert!(matches!(ready_error, RegistryError::CloseFailed { .. }));
    assert_eq!(registry.slot_count(), 0);
}

#[test]
fn close_on_loading_reuses_close_signal_and_transitions_to_closing_after_load() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-1");
    registry
        .begin_load(session_id.clone())
        .expect("reserve load");

    let CloseStart::Loading {
        load_completion,
        close_completion,
        should_spawn,
    } = registry.begin_close(&session_id).expect("close loading")
    else {
        panic!("expected loading close");
    };
    assert!(load_completion.ready().is_none());
    assert!(!close_completion.is_complete());
    assert!(should_spawn);

    let CloseStart::Loading {
        close_completion: repeated_close,
        should_spawn: repeated_should_spawn,
        ..
    } = registry
        .begin_close(&session_id)
        .expect("repeat close loading")
    else {
        panic!("expected repeated loading close");
    };
    assert!(!repeated_close.is_complete());
    assert!(!repeated_should_spawn);

    registry
        .complete_load_success(&session_id, TestHandle("h1"))
        .expect("complete load");

    assert_eq!(registry.get(&session_id), None);
    let CloseStart::Closing { handle, completion } =
        registry.begin_close(&session_id).expect("observe closing")
    else {
        panic!("expected closing after load");
    };
    assert_eq!(handle, TestHandle("h1"));
    assert!(!completion.is_complete());
    assert!(!close_completion.is_complete());
    registry
        .complete_close(&session_id)
        .expect("complete close");
    assert!(close_completion.is_complete());
    assert!(repeated_close.is_complete());
}

#[test]
fn close_on_loading_completes_when_load_fails() {
    let registry = LiveSessionRegistry::<TestHandle>::new(4);
    let session_id = test_session_id("sess-1");
    registry
        .begin_load(session_id.clone())
        .expect("reserve load");
    let CloseStart::Loading {
        close_completion, ..
    } = registry.begin_close(&session_id).expect("close loading")
    else {
        panic!("expected loading close");
    };
    let error = NotFoundSnafu {
        session_id: session_id.clone(),
    }
    .build();

    registry
        .complete_load_failure(&session_id, error)
        .expect("complete load failure");

    assert!(close_completion.is_complete());
    assert_eq!(registry.slot_count(), 0);
}

#[test]
fn list_live_returns_only_live_session_ids() {
    let registry = LiveSessionRegistry::new(4);
    let loading = test_session_id("sess-loading");
    let live = test_session_id("sess-live");
    registry.begin_load(loading).expect("reserve loading slot");
    registry
        .begin_load(live.clone())
        .expect("reserve live slot");
    registry
        .complete_load_success(&live, TestHandle("live"))
        .expect("complete live");

    let live_sessions = registry.list_live();

    assert_eq!(live_sessions, vec![live]);
}

#[test]
fn replace_live_handle_updates_live_slot_and_returns_previous_handle() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-1");
    registry
        .begin_load(session_id.clone())
        .expect("reserve load");
    registry
        .complete_load_success(&session_id, TestHandle("old"))
        .expect("complete load");

    let previous = registry
        .replace_live_handle(&session_id, TestHandle("new"))
        .expect("replace live handle");

    assert_eq!(previous, TestHandle("old"));
    assert_eq!(registry.get(&session_id), Some(TestHandle("new")));
    assert_eq!(registry.live_count(), 1);
}

#[test]
fn replace_live_handle_rejects_non_live_slot() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-loading");
    registry
        .begin_load(session_id.clone())
        .expect("reserve loading");

    let err = registry
        .replace_live_handle(&session_id, TestHandle("new"))
        .expect_err("loading slot is not live");

    assert!(matches!(err, RegistryError::SlotConflict { .. }));
    assert!(matches!(
        registry
            .begin_load(session_id)
            .expect("observe original loading"),
        LoadStart::Loading(_)
    ));
}

#[test]
fn replace_reserves_new_loading_slot_bypassing_max_sessions_by_one() {
    let registry = LiveSessionRegistry::new(1);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    let blocked_session_id = test_session_id("sess-blocked");
    registry
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    registry
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");

    let ReplaceStart::Reserved {
        old_handle,
        new_completion,
        ..
    } = registry
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect("begin replace");

    assert_eq!(old_handle, TestHandle("old"));
    assert!(new_completion.ready().is_none());
    assert_eq!(registry.slot_count(), 2);
    assert_eq!(registry.get(&old_session_id), Some(TestHandle("old")));
    let LoadStart::Loading(observed_new) = registry
        .begin_load(new_session_id)
        .expect("observe new loading")
    else {
        panic!("expected new loading");
    };
    assert!(observed_new.ready().is_none());
    assert!(matches!(
        registry.begin_load(blocked_session_id),
        Err(RegistryError::ResourceExhausted { .. })
    ));
}

#[test]
fn replace_construct_failure_removes_new_and_keeps_old_live() {
    let registry = LiveSessionRegistry::new(1);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    registry
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    registry
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    let ReplaceStart::Reserved { new_completion, .. } = registry
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect("begin replace");
    let error = NotFoundSnafu {
        session_id: new_session_id.clone(),
    }
    .build();

    registry
        .complete_replace_failure(&old_session_id, &new_session_id, error)
        .expect("replace failure");

    let Err(ready_error) = new_completion.ready().expect("new completion ready") else {
        panic!("expected construction error");
    };
    assert!(matches!(ready_error, RegistryError::NotFound { .. }));
    assert_eq!(registry.get(&old_session_id), Some(TestHandle("old")));
    assert_eq!(registry.slot_count(), 1);
}

#[test]
fn replace_commit_promotes_new_and_moves_old_to_closing() {
    let registry = LiveSessionRegistry::new(1);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    registry
        .begin_load(old_session_id.clone())
        .expect("reserve old");
    registry
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    let ReplaceStart::Reserved { new_completion, .. } = registry
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect("begin replace");

    let commit = registry
        .complete_replace_success(&old_session_id, &new_session_id, TestHandle("new"))
        .expect("commit replace");

    assert_eq!(commit.old_handle, TestHandle("old"));
    assert!(!commit.old_close_completion.is_complete());
    assert_eq!(
        new_completion
            .ready()
            .expect("new completion ready")
            .expect("new construction ok"),
        TestHandle("new")
    );
    assert_eq!(registry.get(&old_session_id), None);
    assert_eq!(registry.get(&new_session_id), Some(TestHandle("new")));
    assert_eq!(registry.slot_count(), 2);
    let LoadStart::Closing(old_close_completion) = registry
        .begin_load(old_session_id.clone())
        .expect("observe old closing")
    else {
        panic!("expected old closing");
    };
    assert!(!old_close_completion.is_complete());

    registry
        .complete_close(&old_session_id)
        .expect("finish old close");

    assert!(commit.old_close_completion.is_complete());
    assert_eq!(registry.slot_count(), 1);
    assert_eq!(registry.get(&new_session_id), Some(TestHandle("new")));
}

#[test]
fn replace_requires_live_old_and_unused_new_slot() {
    let registry = LiveSessionRegistry::new(4);
    let old_session_id = test_session_id("sess-old");
    let new_session_id = test_session_id("sess-new");
    registry
        .begin_load(old_session_id.clone())
        .expect("old loading");

    let err = registry
        .begin_replace(&old_session_id, new_session_id.clone())
        .expect_err("old must be live");
    assert!(matches!(err, RegistryError::OldNotReady { .. }));

    registry
        .complete_load_success(&old_session_id, TestHandle("old"))
        .expect("old live");
    registry
        .begin_load(new_session_id.clone())
        .expect("new occupied");
    let err = registry
        .begin_replace(&old_session_id, new_session_id)
        .expect_err("new slot must be unused");
    assert!(matches!(err, RegistryError::NewSlotOccupied { .. }));
}

#[test]
fn announced_membership_retains_closing_slots_while_live_only_excludes_them() {
    // CS-4 / R17: a Closing session must stay in `list_announced` (the set the
    // Event Hub announces to process egress) until the close cascade removes
    // its slot, so a reconnect during the Closing window still negotiates a
    // resume cursor for it via the announce ack. The live-only projection drops
    // it at the same instant.
    let registry = LiveSessionRegistry::new(4);
    let live = test_session_id("sess-live");
    let closing = test_session_id("sess-closing");
    for id in [&live, &closing] {
        registry.begin_load(id.clone()).expect("reserve load");
        registry
            .complete_load_success(id, TestHandle("h"))
            .expect("promote to live");
    }

    let sorted = |mut ids: Vec<SessionId>| {
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ids
    };
    let both = sorted(vec![closing.clone(), live.clone()]);
    assert_eq!(sorted(registry.list_announced()), both);
    assert_eq!(sorted(registry.list_live()), both);

    // One session enters its Closing window.
    registry.begin_close(&closing).expect("begin close");

    // Announced membership still covers BOTH — the closing one is retiring, so
    // reconnect cursor negotiation includes it — while the live-only projection
    // drops it immediately.
    assert_eq!(sorted(registry.list_announced()), both);
    assert_eq!(registry.list_live(), vec![live]);
}

fn sorted_ids(mut ids: Vec<SessionId>) -> Vec<SessionId> {
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    ids
}

#[test]
fn begin_child_load_reserves_internal_local_only_child() {
    let registry = LiveSessionRegistry::new(2);
    let parent = test_session_id("parent");
    let child = test_session_id("child");
    registry.begin_load(parent.clone()).expect("reserve parent");
    registry
        .complete_load_success(&parent, TestHandle("p"))
        .expect("parent live");

    assert!(matches!(
        registry
            .begin_child_load(&parent, child.clone())
            .expect("reserve child"),
        LoadStart::Reserved
    ));
    registry
        .complete_load_success(&child, TestHandle("c"))
        .expect("child live");

    assert_eq!(registry.child_of(&parent), Some(child.clone()));
    let policy = registry.policy(&child).expect("child policy");
    assert!(policy.is_internal());
    assert!(policy.is_local_only());
    assert_eq!(policy.parent(), Some(&parent));
    // Public visibility: parent yes, child no.
    assert!(registry.is_public(&parent));
    assert!(!registry.is_public(&child));
    // Public/live and announced projections exclude the internal child; the
    // raw list_live keeps it (internal routing needs the handle).
    assert_eq!(registry.list_public_live(), vec![parent.clone()]);
    assert_eq!(registry.list_announced(), vec![parent.clone()]);
    assert_eq!(
        sorted_ids(registry.list_live()),
        sorted_ids(vec![child, parent])
    );
}

#[test]
fn begin_child_load_rejects_second_child_and_requires_live_parent() {
    let registry = LiveSessionRegistry::new(4);
    let parent = test_session_id("parent");
    let loading_parent = test_session_id("loading-parent");
    let child1 = test_session_id("child-1");
    let child2 = test_session_id("child-2");

    // Parent must be live to own a child.
    registry
        .begin_load(loading_parent.clone())
        .expect("reserve loading parent");
    assert!(matches!(
        registry.begin_child_load(&loading_parent, child1.clone()),
        Err(RegistryError::OldNotReady { .. })
    ));

    registry.begin_load(parent.clone()).expect("reserve parent");
    registry
        .complete_load_success(&parent, TestHandle("p"))
        .expect("parent live");
    registry
        .begin_child_load(&parent, child1)
        .expect("reserve first child");
    // A second child while the first is still loading is rejected.
    assert!(matches!(
        registry.begin_child_load(&parent, child2),
        Err(RegistryError::ChildExists { .. })
    ));
}

#[test]
fn child_close_clears_index_and_allows_a_new_child() {
    let registry = LiveSessionRegistry::new(2);
    let parent = test_session_id("parent");
    let child = test_session_id("child");
    let child2 = test_session_id("child-2");
    registry.begin_load(parent.clone()).expect("reserve parent");
    registry
        .complete_load_success(&parent, TestHandle("p"))
        .expect("parent live");
    registry
        .begin_child_load(&parent, child.clone())
        .expect("reserve child");
    registry
        .complete_load_success(&child, TestHandle("c"))
        .expect("child live");

    registry.begin_close(&child).expect("begin child close");
    registry.complete_close(&child).expect("finish child close");

    // Terminal close cleared the slot, policy, and parent→child index.
    assert_eq!(registry.child_of(&parent), None);
    assert!(registry.policy(&child).is_none());
    assert_eq!(registry.slot_count(), 1);
    // A replacement child may now be reserved.
    assert!(matches!(
        registry
            .begin_child_load(&parent, child2.clone())
            .expect("reserve replacement child"),
        LoadStart::Reserved
    ));
    assert_eq!(registry.child_of(&parent), Some(child2));
}

#[test]
fn child_load_failure_clears_index() {
    let registry = LiveSessionRegistry::<TestHandle>::new(2);
    let parent = test_session_id("parent");
    let child = test_session_id("child");
    registry.begin_load(parent.clone()).expect("reserve parent");
    registry
        .complete_load_success(&parent, TestHandle("p"))
        .expect("parent live");
    registry
        .begin_child_load(&parent, child.clone())
        .expect("reserve child");
    let error = NotFoundSnafu {
        session_id: child.clone(),
    }
    .build();

    registry
        .complete_load_failure(&child, error)
        .expect("child load failure");

    assert_eq!(registry.child_of(&parent), None);
    assert!(registry.policy(&child).is_none());
    assert_eq!(registry.slot_count(), 1);
}

#[test]
fn begin_delete_blocks_slot_reservation_until_finished() {
    let registry = LiveSessionRegistry::<TestHandle>::new(4);
    let session_id = test_session_id("sess-deleting");

    registry.begin_delete(&session_id).expect("begin delete");
    assert!(matches!(
        registry.begin_delete(&session_id),
        Err(RegistryError::DeleteInProgress { .. })
    ));
    assert!(matches!(
        registry.begin_load(session_id.clone()),
        Err(RegistryError::DeleteInProgress { .. })
    ));

    registry.finish_delete(&session_id);
    assert!(matches!(
        registry.begin_load(session_id).expect("reserve"),
        LoadStart::Reserved
    ));
}

#[test]
fn begin_delete_rejects_an_existing_slot() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-delete-live");
    assert!(matches!(
        registry
            .begin_load(session_id.clone())
            .expect("reserve load"),
        LoadStart::Reserved
    ));
    registry
        .complete_load_success(&session_id, TestHandle("live"))
        .expect("complete load");

    assert!(matches!(
        registry.begin_delete(&session_id),
        Err(RegistryError::SlotConflict { .. })
    ));
}

#[test]
fn complete_load_success_returns_the_handle_when_the_slot_is_gone() {
    let registry = LiveSessionRegistry::new(4);
    let session_id = test_session_id("sess-load-conflict");
    assert!(matches!(
        registry
            .begin_load(session_id.clone())
            .expect("reserve load"),
        LoadStart::Reserved
    ));
    registry
        .complete_load_failure(&session_id, RegistryError::load_failed("aborted"))
        .expect("fail load");

    // The commit lost its slot: the constructed handle must come back to the
    // owner for teardown instead of being dropped.
    let failure = registry
        .complete_load_success(&session_id, TestHandle("constructed"))
        .expect_err("slot is gone");
    assert_eq!(failure.handle, TestHandle("constructed"));
    assert!(matches!(failure.error, RegistryError::SlotConflict { .. }));
}

#[test]
fn complete_replace_failure_unblocks_child_admission_even_on_slot_conflict() {
    let registry = LiveSessionRegistry::new(4);
    let parent = test_session_id("sess-replace-parent");
    assert!(matches!(
        registry.begin_load(parent.clone()).expect("reserve load"),
        LoadStart::Reserved
    ));
    registry
        .complete_load_success(&parent, TestHandle("parent"))
        .expect("parent live");
    let replacement = test_session_id("sess-replace-new");
    let ReplaceStart::Reserved { .. } = registry
        .begin_replace(&parent, replacement.clone())
        .expect("begin replace");

    // Simulate the replacement slot vanishing before the failure completes:
    // the unblock must still run or the parent can never admit a sidechat.
    registry
        .complete_load_failure(&replacement, RegistryError::load_failed("factory failed"))
        .expect("fail replacement");
    let _ = registry.complete_replace_failure(
        &parent,
        &replacement,
        RegistryError::load_failed("factory failed"),
    );

    let child = test_session_id("sess-replace-child");
    assert!(matches!(
        registry
            .begin_child_load(&parent, child)
            .expect("child admission unblocked"),
        LoadStart::Reserved
    ));
}
