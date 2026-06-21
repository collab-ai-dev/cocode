use super::*;

fn make_runner(max_agents: i32) -> InProcessAgentRunner {
    InProcessAgentRunner::new("/tmp/test".into(), max_agents)
}

fn make_spawn_config(name: &str, team: &str) -> SpawnConfig {
    SpawnConfig {
        name: name.into(),
        team_name: team.into(),
        prompt: "test prompt".into(),
        color: None,
        plan_mode_required: false,
        model: None,
        working_dir: None,
        system_prompt: None,
        allowed_tools: vec![],
        allow_permission_prompts: false,
        effort: None,
        use_exact_tools: false,
        isolation: coco_types::AgentIsolation::None,
        memory_scope: None,
        mcp_servers: vec![],
        disallowed_tools: vec![],
        max_turns: None,
    }
}

#[tokio::test]
async fn test_spawn_agent_success() {
    let runner = make_runner(5);
    let result = runner
        .register_agent(make_spawn_config("researcher", "team-a"))
        .await;

    assert!(result.success);
    assert_eq!(result.agent_id, "researcher@team-a");
    assert!(result.error.is_none());

    let active: Vec<AgentContext> = runner.active_agents().await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].agent_name, "researcher");
}

#[tokio::test]
async fn test_spawn_agent_duplicate_rejected() {
    let runner = make_runner(5);
    let r1 = runner
        .register_agent(make_spawn_config("worker", "team-a"))
        .await;
    assert!(r1.success);

    let r2 = runner
        .register_agent(make_spawn_config("worker", "team-a"))
        .await;
    assert!(!r2.success);
    assert!(
        r2.error
            .as_ref()
            .is_some_and(|e: &String| e.contains("already exists"))
    );
}

#[tokio::test]
async fn test_spawn_agent_max_capacity() {
    let runner = make_runner(1);
    let r1 = runner.register_agent(make_spawn_config("a1", "team")).await;
    assert!(r1.success);

    let r2 = runner.register_agent(make_spawn_config("a2", "team")).await;
    assert!(!r2.success);
    assert!(
        r2.error
            .as_ref()
            .is_some_and(|e: &String| e.contains("Max agents"))
    );
}

#[tokio::test]
async fn test_cancel_agent() {
    let runner = make_runner(5);
    let result = runner
        .register_agent(make_spawn_config("agent-x", "team"))
        .await;
    assert!(result.success);

    let cancelled = runner.cancel_agent("agent-x@team").await;
    assert!(cancelled);

    // Agent should be removed
    assert_eq!(runner.active_count().await, 0);

    // Cancelling again should return false
    assert!(!runner.cancel_agent("agent-x@team").await);
}

#[tokio::test]
async fn test_cancel_all_agents() {
    let runner = make_runner(5);
    runner.register_agent(make_spawn_config("a1", "team")).await;
    runner.register_agent(make_spawn_config("a2", "team")).await;
    assert_eq!(runner.active_count().await, 2);

    runner.cancel_all().await;
    assert_eq!(runner.active_count().await, 0);
}

#[tokio::test]
async fn test_start_agent_forwards_join_handle_to_collect_result() {
    use crate::runner_loop::InProcessRunnerResult;
    let runner = make_runner(5);
    let spawn = runner
        .register_agent(make_spawn_config("worker", "team"))
        .await;
    assert!(spawn.success);

    // Simulate the execution loop: spawn a task that produces an
    // InProcessRunnerResult. The runner's start_agent takes ownership
    // of the JoinHandle and forwards it into the oneshot.
    let handle = tokio::spawn(async move {
        InProcessRunnerResult {
            success: true,
            error: None,
            output: Some("task done".into()),
            turns: 3,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    });

    let started = runner.start_agent("worker@team", handle).await;
    assert!(started, "start_agent must succeed for a registered agent");

    let result: Option<RunnerResult> = runner.collect_result("worker@team").await;
    let r = result.expect("result exists");
    assert!(r.success);
    assert_eq!(r.output.as_deref(), Some("task done"));
    assert_eq!(r.turns, 3);
}

#[tokio::test]
async fn test_start_agent_returns_false_for_unknown_id() {
    use crate::runner_loop::InProcessRunnerResult;
    let runner = make_runner(5);
    let handle = tokio::spawn(async {
        InProcessRunnerResult {
            success: false,
            error: Some("never registered".into()),
            output: None,
            turns: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    });
    let started = runner.start_agent("does-not-exist@team", handle).await;
    assert!(!started, "start_agent must reject unregistered agent_id");
}

#[tokio::test]
async fn test_start_agent_panicked_handle_produces_error_result() {
    let runner = make_runner(5);
    let spawn = runner
        .register_agent(make_spawn_config("worker", "team"))
        .await;
    assert!(spawn.success);

    let handle = tokio::spawn(async { panic!("agent exploded") });
    runner.start_agent("worker@team", handle).await;

    let r = runner
        .collect_result("worker@team")
        .await
        .expect("result arrives even on panic");
    assert!(!r.success);
    assert!(
        r.error.as_deref().unwrap_or("").contains("panicked"),
        "panic must be surfaced via RunnerResult::error; got: {:?}",
        r.error
    );
}

#[tokio::test]
async fn test_get_context() {
    let runner = make_runner(5);
    runner
        .register_agent(SpawnConfig {
            name: "ctx-agent".into(),
            team_name: "ctx-team".into(),
            prompt: "hello".into(),
            color: Some("blue".into()),
            plan_mode_required: true,
            model: Some("anthropic/claude-opus-4-7".into()),
            working_dir: Some("/custom/dir".into()),
            system_prompt: Some("You are helpful.".into()),
            allowed_tools: vec!["Read".into(), "Write".into()],
            allow_permission_prompts: true,
            effort: Some(coco_types::ReasoningEffort::High),
            use_exact_tools: true,
            isolation: coco_types::AgentIsolation::Worktree,
            memory_scope: Some(coco_types::MemoryScope::Project),
            mcp_servers: vec!["github".into()],
            disallowed_tools: vec!["Bash".into()],
            max_turns: Some(15),
        })
        .await;

    let ctx: Option<AgentContext> = runner.get_context("ctx-agent@ctx-team").await;
    assert!(ctx.is_some());
    let ctx = ctx.expect("context exists");
    assert_eq!(ctx.agent_id, "ctx-agent@ctx-team");
    assert_eq!(ctx.color.as_deref(), Some("blue"));
    assert!(ctx.plan_mode_required);
    assert_eq!(ctx.model.as_deref(), Some("anthropic/claude-opus-4-7"));
    assert_eq!(ctx.working_dir, "/custom/dir");
    assert_eq!(ctx.allowed_tools, vec!["Read", "Write"]);
    assert!(ctx.allow_permission_prompts);
    assert_eq!(ctx.effort, Some(coco_types::ReasoningEffort::High));
    assert!(ctx.use_exact_tools);
    assert_eq!(ctx.isolation, coco_types::AgentIsolation::Worktree);
    assert_eq!(ctx.memory_scope, Some(coco_types::MemoryScope::Project));
    assert_eq!(ctx.mcp_servers, vec!["github"]);
    assert_eq!(ctx.disallowed_tools, vec!["Bash"]);
    assert_eq!(ctx.max_turns, Some(15));
}

#[tokio::test]
async fn test_collect_nonexistent_agent() {
    let runner = make_runner(5);
    let result: Option<RunnerResult> = runner.collect_result("nonexistent@team").await;
    assert!(result.is_none());
}

// `test_permission_bridge_resolve` removed: the in-process mpsc
// `runner::PermissionBridge` was orphaned after Phase D — no
// production path enqueued onto it. In-process workers inherit the
// leader's `ToolPermissionBridge` via `wire_engine`; cross-process
// pane workers use `MailboxPermissionBridge`. The dead circuit was
// deleted during the post-D cleanup pass.
