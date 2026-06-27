//! Tests for `InProcessBackend` — the trait-object wrapper around
//! `InProcessAgentRunner` that backs the `BackendRegistry`'s
//! `InProcess` variant.

use std::sync::Arc;

use super::*;
use crate::pane::SystemPromptMode;

fn make_backend() -> InProcessBackend {
    let runner = Arc::new(runner::InProcessAgentRunner::new(
        "/tmp".into(),
        /*max_agents*/ 8,
    ));
    InProcessBackend::new(runner)
}

fn spawn_config(name: &str, team: &str) -> TeammateSpawnConfig {
    TeammateSpawnConfig {
        name: name.into(),
        team_name: team.into(),
        prompt: "do work".into(),
        color: None,
        plan_mode_required: false,
        model: None,
        cwd: "/tmp".into(),
        system_prompt: None,
        system_prompt_mode: SystemPromptMode::Default,
        worktree_path: None,
        parent_session_id: "session-x".into(),
        permissions: Vec::new(),
        allow_permission_prompts: false,
        effort: None,
        use_exact_tools: false,
        mcp_servers: Vec::new(),
        disallowed_tools: Vec::new(),
        max_turns: None,
    }
}

#[tokio::test]
async fn test_backend_type_is_in_process() {
    let backend = make_backend();
    assert_eq!(backend.backend_type(), BackendType::InProcess);
}

#[tokio::test]
async fn test_is_available_always_true() {
    // The in-process backend has no external dependency to detect —
    // it's always available, unlike tmux/iTerm2 which need their
    // host program installed.
    assert!(make_backend().is_available().await);
}

#[tokio::test]
async fn test_spawn_returns_success_with_agent_id_and_task_id() {
    let backend = make_backend();
    let result = backend.spawn(spawn_config("worker", "alpha")).await;
    assert!(result.success, "expected success, got {result:?}");
    assert_eq!(result.agent_id, "worker@alpha");
    // task_id is the spawn correlation handle the registry hands
    // back to the orchestrator.
    let task_id = result.task_id.expect("task_id must be set on success");
    assert!(
        task_id.starts_with("task-"),
        "task_id must use the `task-` prefix: {task_id}"
    );
    assert_eq!(result.pane_id, None, "in-process has no terminal pane");
}

#[tokio::test]
async fn test_spawn_failure_propagates_error() {
    // Two agents with the same `name@team` collide — the second
    // registration must fail and surface the error to the executor.
    let backend = make_backend();
    let _ok = backend.spawn(spawn_config("worker", "alpha")).await;
    let result = backend.spawn(spawn_config("worker", "alpha")).await;
    assert!(!result.success, "duplicate spawn must fail: {result:?}");
    assert!(result.error.is_some(), "error message must surface");
    assert!(
        result.task_id.is_none(),
        "task_id must be None on failure (caller wouldn't have anything to address)"
    );
}

#[tokio::test]
async fn test_is_active_after_spawn_and_after_kill() {
    let backend = make_backend();
    let _ = backend.spawn(spawn_config("scout", "team-a")).await;
    assert!(backend.is_active("scout@team-a").await);

    // Kill cancels the cancellation token; is_active reads it.
    let killed = backend.kill("scout@team-a").await.unwrap();
    assert!(killed, "kill returns true when an agent existed");
    assert!(!backend.is_active("scout@team-a").await);
}

#[tokio::test]
async fn test_is_active_returns_false_for_unknown_agent() {
    let backend = make_backend();
    assert!(!backend.is_active("ghost@team-z").await);
}
