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
        .complete_replace_failure(&new_session_id, error)
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
