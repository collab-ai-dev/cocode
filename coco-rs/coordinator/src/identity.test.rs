use super::*;

#[test]
fn test_create_teammate_context() {
    let ctx = create_teammate_context("researcher", "my-team", Some("blue".into()), true, "sess-1");
    assert_eq!(ctx.agent_id, "researcher@my-team");
    assert_eq!(ctx.agent_name, "researcher");
    assert_eq!(ctx.team_name, "my-team");
    assert_eq!(ctx.color.as_deref(), Some("blue"));
    assert!(ctx.plan_mode_required);
    assert_eq!(ctx.parent_session_id, "sess-1");
}

#[tokio::test]
async fn test_dynamic_context_set_clear() {
    // `DYNAMIC_CONTEXT` is a process-global; serialize against other identity
    // tests (and readers like `test_send_message_no_team`) under threaded
    // `cargo test`.
    let _env = crate::test_support::lock_env().await;
    clear_dynamic_team_context();
    assert!(get_dynamic_team_context().is_none());

    set_dynamic_team_context(DynamicTeamContext {
        agent_id: "worker@team".into(),
        agent_name: "worker".into(),
        team_name: "team".into(),
        color: None,
        plan_mode_required: false,
        parent_session_id: None,
    });

    let ctx = get_dynamic_team_context().unwrap();
    assert_eq!(ctx.agent_id, "worker@team");

    clear_dynamic_team_context();
    assert!(get_dynamic_team_context().is_none());
}

#[tokio::test]
async fn test_resolve_teammate_identity_from_dynamic_context() {
    let _env = crate::test_support::lock_env().await;
    set_dynamic_team_context(DynamicTeamContext {
        agent_id: "researcher@my-team".into(),
        agent_name: "researcher".into(),
        team_name: "my-team".into(),
        color: Some("blue".into()),
        plan_mode_required: true,
        parent_session_id: None,
    });

    let id = resolve_teammate_identity().expect("teammate identity resolves from context");
    assert_eq!(id.agent_id, "researcher@my-team");
    assert_eq!(id.agent_name, "researcher");
    assert_eq!(id.team_name, "my-team");
    assert_eq!(id.color, Some(coco_types::AgentColorName::Blue));
    assert!(id.plan_mode_required);

    clear_dynamic_team_context();
}

/// Tier-3 (env-var) identity resolution. This is the path that makes a spawned
/// teammate's `--resume` safe — the resumed process inherits `COCO_*` from its
/// pane/parent — and therefore the reason deleting the transcript-scan
/// `reconnect.rs` was a behavioral no-op (R3 was a false-positive). Locks the
/// supported resume topology against regression.
#[tokio::test]
async fn test_resolve_teammate_identity_from_env_vars() {
    use crate::constants::AGENT_ID_ENV_VAR;
    use crate::constants::AGENT_NAME_ENV_VAR;
    use crate::constants::PLAN_MODE_REQUIRED_ENV_VAR;
    use crate::constants::TEAM_NAME_ENV_VAR;
    use crate::constants::TEAMMATE_COLOR_ENV_VAR;

    // Serialize against other env-touching tests (readers like
    // `test_send_message_no_team` resolve identity from these same globals).
    let _env = crate::test_support::lock_env().await;

    // Remove the identity env on drop even if an assert panics, so the next
    // test that acquires `ENV_LOCK` starts from a clean environment.
    struct ClearIdentityEnv;
    impl Drop for ClearIdentityEnv {
        fn drop(&mut self) {
            // SAFETY: serialized via the held `ENV_LOCK` guard.
            unsafe {
                std::env::remove_var(AGENT_ID_ENV_VAR.as_str());
                std::env::remove_var(AGENT_NAME_ENV_VAR.as_str());
                std::env::remove_var(TEAM_NAME_ENV_VAR.as_str());
                std::env::remove_var(TEAMMATE_COLOR_ENV_VAR.as_str());
                std::env::remove_var(PLAN_MODE_REQUIRED_ENV_VAR.as_str());
            }
        }
    }
    let _clear = ClearIdentityEnv;

    // Tier-1 (task-local) is absent outside a `run_with_teammate_context`
    // scope; clear tier-2 so resolution falls through to the env tier.
    clear_dynamic_team_context();
    // SAFETY: serialized via the held `ENV_LOCK` guard; nextest isolates per
    // process.
    unsafe {
        std::env::set_var(AGENT_ID_ENV_VAR.as_str(), "researcher@my-team");
        std::env::set_var(AGENT_NAME_ENV_VAR.as_str(), "researcher");
        std::env::set_var(TEAM_NAME_ENV_VAR.as_str(), "my-team");
        std::env::set_var(TEAMMATE_COLOR_ENV_VAR.as_str(), "blue");
        std::env::set_var(PLAN_MODE_REQUIRED_ENV_VAR.as_str(), "1");
    }

    let id = resolve_teammate_identity().expect("identity resolves from env (resume path)");
    assert_eq!(id.agent_id, "researcher@my-team");
    assert_eq!(id.agent_name, "researcher");
    assert_eq!(id.team_name, "my-team");
    assert_eq!(id.color, Some(coco_types::AgentColorName::Blue));
    assert!(id.plan_mode_required);
}

#[tokio::test]
async fn test_task_local_context() {
    let ctx = create_teammate_context("tester", "t", None, false, "s");

    let result = run_with_teammate_context(ctx, async {
        let inner = get_teammate_context().unwrap();
        assert_eq!(inner.agent_name, "tester");
        assert!(is_in_process_teammate());
        42
    })
    .await;
    assert_eq!(result, 42);

    // Outside the scope, should not have context
    assert!(!is_in_process_teammate());
}
