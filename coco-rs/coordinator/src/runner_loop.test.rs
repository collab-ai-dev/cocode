//! Smoke tests for `runner_loop` after the task-storage unification.
//!
//! The pre-refactor suite exercised the deleted `InProcessTeammateTaskState`
//! mirror — every assertion read `state.is_idle`, `state.messages`,
//! `state.current_work_cancel` from the parallel store the
//! task-storage refactor removed. Those tests are no longer meaningful
//! against the unified `TaskManager`-only model; their replacements
//! live in `tasks/running.test.rs` (canonical row + control-handle
//! sibling map) and `agent_handle/mod.test.rs` (teammate dispatch).
//!
//! This file keeps the no-arg helpers (`AgentQueryConfig::default`,
//! `WaitResult` shape checks) as compile-time tripwires so accidental
//! API changes there fail loudly.

use super::*;

#[test]
fn agent_query_config_default_is_constructible() {
    let cfg = AgentQueryConfig::default();
    assert!(cfg.system_prompt.is_empty());
    assert!(cfg.allowed_tools.is_empty());
    assert!(cfg.disallowed_tools.is_empty());
    assert!(cfg.fork_context_messages.is_empty());
    assert!(cfg.cancel.is_none());
}

#[test]
fn wait_result_aborted_is_constructible() {
    let r = WaitResult::Aborted;
    assert!(matches!(r, WaitResult::Aborted));
}

// ── select_mailbox_prompt: priority + filter (pure, no I/O) ──

fn msg(from: &str, text: &str, read: bool) -> mailbox::TeammateMessage {
    mailbox::TeammateMessage {
        from: from.to_string(),
        text: text.to_string(),
        timestamp: "2026-06-04T00:00:00Z".to_string(),
        read,
        color: None,
        summary: None,
    }
}

fn shutdown_text() -> String {
    serde_json::to_string(&mailbox::ProtocolMessage::ShutdownRequest {
        request_id: "shutdown-1".to_string(),
        from: TEAM_LEAD_NAME.to_string(),
        reason: None,
        timestamp: "2026-06-04T00:00:00Z".to_string(),
    })
    .unwrap()
}

fn mode_set_text() -> String {
    serde_json::to_string(&mailbox::ProtocolMessage::ModeSetRequest {
        mode: coco_types::PermissionMode::Plan,
        from: TEAM_LEAD_NAME.to_string(),
    })
    .unwrap()
}

fn plan_approval_response_text() -> String {
    serde_json::to_string(&mailbox::ProtocolMessage::PlanApprovalResponse {
        request_id: "plan-1".to_string(),
        approved: true,
        feedback: None,
        timestamp: String::new(),
        permission_mode: None,
    })
    .unwrap()
}

#[test]
fn select_mailbox_prompt_shutdown_outranks_text() {
    let messages = vec![
        msg(TEAM_LEAD_NAME, "do the thing", false),
        msg(TEAM_LEAD_NAME, &shutdown_text(), false),
        msg("researcher", "peer note", false),
    ];
    let (idx, result) = select_mailbox_prompt(&messages).expect("a prompt");
    assert_eq!(idx, 1);
    assert!(matches!(result, WaitResult::ShutdownRequest { .. }));
}

#[test]
fn select_mailbox_prompt_team_lead_outranks_peer_regardless_of_order() {
    // Peer message appears first, but the team-lead arm wins.
    let messages = vec![
        msg("researcher", "peer note", false),
        msg(TEAM_LEAD_NAME, "leader task", false),
    ];
    let (idx, result) = select_mailbox_prompt(&messages).expect("a prompt");
    assert_eq!(idx, 1);
    match result {
        WaitResult::NewMessage { message, from, .. } => {
            assert_eq!(from, TEAM_LEAD_NAME);
            assert_eq!(message, "leader task");
        }
        other => panic!("expected NewMessage, got {other:?}"),
    }
}

#[test]
fn select_mailbox_prompt_peer_is_fifo() {
    let messages = vec![msg("alice", "first", false), msg("bob", "second", false)];
    let (idx, result) = select_mailbox_prompt(&messages).expect("a prompt");
    assert_eq!(idx, 0);
    match result {
        WaitResult::NewMessage { from, message, .. } => {
            assert_eq!(from, "alice");
            assert_eq!(message, "first");
        }
        other => panic!("expected NewMessage, got {other:?}"),
    }
}

#[test]
fn select_mailbox_prompt_skips_structured_responses() {
    // A control message + a response message in the teammate's own inbox must
    // NOT be injected as prompts (gap-1 mis-injection guard). Neither is a
    // ShutdownRequest, so the scan yields nothing.
    let messages = vec![
        msg(TEAM_LEAD_NAME, &mode_set_text(), false),
        msg(TEAM_LEAD_NAME, &plan_approval_response_text(), false),
    ];
    assert!(select_mailbox_prompt(&messages).is_none());
}

#[test]
fn select_mailbox_prompt_skips_read_and_empty() {
    assert!(select_mailbox_prompt(&[]).is_none());
    let messages = vec![msg(TEAM_LEAD_NAME, "already handled", true)];
    assert!(select_mailbox_prompt(&messages).is_none());
}

#[test]
fn select_mailbox_prompt_plain_text_alongside_structured_picks_text() {
    // A real leader prompt arriving after a response message is still found.
    let messages = vec![
        msg(TEAM_LEAD_NAME, &plan_approval_response_text(), false),
        msg(TEAM_LEAD_NAME, "now do step 2", false),
    ];
    let (idx, result) = select_mailbox_prompt(&messages).expect("a prompt");
    assert_eq!(idx, 1);
    assert!(matches!(result, WaitResult::NewMessage { .. }));
}

// ── In-process control consumer (gap 8) ──
//
// The symmetric analog of the cross-process pump's `drain_control_tick` test
// (`app/cli/teammate_inbox_pump.test.rs`): the IN-PROCESS teammate applies a
// leader `ModeSetRequest` via `drain_control_messages` mutating its live
// `TeammateControlState`, where the cross-process teammate injects a
// `SetPermissionMode` command. Driven here against a REAL on-disk mailbox
// isolated via `COCO_TEAMS_DIR` (set under `<tmp>/teams` so it stays harmless
// to the `teams_base_dir` path-assertion tests even under bare `cargo test`;
// nextest isolates per process regardless).

// Shared crate-wide `COCO_TEAMS_DIR` isolation (one `ENV_LOCK` across all test
// modules — see `crate::test_support`).
use crate::test_support::isolate_teams_dir;

#[tokio::test]
async fn in_process_drain_applies_mode_set_against_real_mailbox() {
    let _teams = isolate_teams_dir().await;

    let identity = TeammateIdentity {
        agent_id: "worker@t".to_string(),
        agent_name: "worker".to_string(),
        team_name: "t".to_string(),
        color: None,
        plan_mode_required: false,
    };

    // Leader writes a ModeSetRequest into the in-process teammate's mailbox.
    mailbox::write_to_mailbox(
        &identity.agent_name,
        mailbox::TeammateMessage {
            from: TEAM_LEAD_NAME.to_string(),
            text: mailbox::create_mode_set_request(
                coco_types::PermissionMode::Plan,
                TEAM_LEAD_NAME,
            ),
            timestamp: "t".to_string(),
            read: false,
            color: None,
            summary: None,
        },
        &identity.team_name,
    )
    .unwrap();

    let control = RwLock::new(TeammateControlState {
        permission_mode: Arc::new(RwLock::new(coco_types::PermissionMode::Default)),
        team_permission_rules: Arc::new(RwLock::new(Vec::new())),
    });
    drain_control_messages(&identity, &control).await;

    // The teammate's LIVE permission mode flipped to Plan…
    let mode_arc = control.read().await.permission_mode.clone();
    assert_eq!(*mode_arc.read().await, coco_types::PermissionMode::Plan);
    // …and the consumed control message is marked read.
    let msgs = mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap();
    assert!(
        msgs.iter().all(|m| m.read),
        "the consumed control message must be marked read"
    );
}

// ── In-process turn loop (the L2-analog) ──
//
// The in-process counterpart of the cross-process L2 (`teammate_pty_e2e.rs`),
// but in-process via a mock engine instead of a real binary + PTY + wiremock:
// drive `run_in_process_teammate` with a recording `AgentExecutionEngine`,
// assert the runner ran a turn carrying the initial prompt, and that a
// leader `ShutdownRequest` seeded in the mailbox terminates the loop cleanly.

/// Records every prompt the runner hands to the engine; returns a trivial
/// one-turn result so the loop advances. When a turn's prompt contains
/// `approve_marker`, the mock calls `signal_self_stop` to simulate the model
/// approving a shutdown via `SendMessageTool` → `respond_to_shutdown`.
struct RecordingEngine {
    prompts: Arc<tokio::sync::Mutex<Vec<String>>>,
    /// Substring that triggers a simulated shutdown approval.
    approve_marker: String,
}

#[async_trait::async_trait]
impl AgentExecutionEngine for RecordingEngine {
    async fn run_query(
        &self,
        prompt: &str,
        _config: AgentQueryConfig,
    ) -> crate::Result<AgentQueryResult> {
        self.prompts.lock().await.push(prompt.to_string());
        // Simulate the model APPROVING shutdown: the real `SendMessageTool`
        // calls `respond_to_shutdown` → `signal_self_stop`. We run inside the
        // runner's task-local scope (the `run_with_teammate_context` wrap),
        // so this flips `config.cancelled` exactly as production would.
        if prompt.contains(&self.approve_marker) {
            assert!(
                crate::identity::signal_self_stop(),
                "in-process teammate turn must run inside a task-local scope \
                 with a wired self_stop_signal"
            );
        }
        Ok(AgentQueryResult {
            messages: Vec::new(),
            token_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            turns: 1,
            tool_use_count: 0,
            cancelled: false,
            response_text: Some("ok".to_string()),
        })
    }
}

fn in_process_config(prompt: &str) -> InProcessRunnerConfig {
    InProcessRunnerConfig {
        identity: TeammateIdentity {
            agent_id: "worker@t".to_string(),
            agent_name: "worker".to_string(),
            team_name: "t".to_string(),
            color: None,
            plan_mode_required: false,
        },
        task_id: "task-1".to_string(),
        session_id: "session-1".to_string(),
        prompt: prompt.to_string(),
        model: None,
        system_prompt: None,
        system_prompt_mode: SystemPromptMode::Default,
        allowed_tools: Vec::new(),
        allow_permission_prompts: false,
        max_turns: Some(1),
        cancelled: Arc::new(AtomicBool::new(false)),
        auto_compact_threshold: 1_000_000,
        bypass_permissions_available: false,
        features: None,
        tool_overrides: None,
        parent_tool_filter: None,
        active_shell_tool: coco_types::ActiveShellTool::Disabled,
        effort: None,
        use_exact_tools: false,
        mcp_servers: Vec::new(),
        disallowed_tools: Vec::new(),
        model_role: None,
        model_selection: coco_types::LlmModelSelection::default(),
        task_list: None,
        task_registry: None,
        roster_store: None,
        plan_mode_required: false,
        hooks: None,
        orchestration_ctx: None,
    }
}

#[tokio::test]
async fn in_process_teammate_runs_initial_prompt_then_exits_on_shutdown() {
    let _teams = isolate_teams_dir().await;

    const MARKER: &str = "RUN_INPROC_MARKER_7";

    // Seed a leader ShutdownRequest so the loop exits after the initial turn.
    mailbox::send_shutdown_request("worker", "t", TEAM_LEAD_NAME, Some("done")).unwrap();

    let prompts = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let engine = RecordingEngine {
        prompts: prompts.clone(),
        // Approve when the shutdown turn arrives.
        approve_marker: "summary=\"shutdown request\"".to_string(),
    };

    let result =
        run_in_process_teammate(in_process_config(&format!("please {MARKER}")), &engine).await;

    let recorded = prompts.lock().await.clone();
    assert!(
        !recorded.is_empty(),
        "the in-process runner must drive at least one turn"
    );
    assert!(
        recorded[0].contains(MARKER),
        "the first turn must carry the initial prompt; got: {}",
        recorded[0]
    );
    assert!(
        result.success,
        "clean shutdown should report success; error={:?}",
        result.error
    );
}

/// A REJECTED shutdown must NOT terminate the in-process teammate — it keeps
/// working and processes the next message. Regression guard for the
/// removal of the old unconditional `handling_shutdown` break, which exited
/// the loop on EVERY shutdown turn regardless of the model's decision (TS
/// `inProcessRunner.ts` only exits when the model approves).
#[tokio::test]
async fn in_process_teammate_rejecting_shutdown_keeps_working() {
    let _teams = isolate_teams_dir().await;

    const FOLLOWUP: &str = "KEEP_WORKING_MARKER_9";

    // Seed a shutdown request (the model will reject it) AND a follow-up
    // task message. The shutdown is prioritized first; after the rejection
    // the loop must continue and pick up the follow-up.
    mailbox::send_shutdown_request("worker", "t", TEAM_LEAD_NAME, Some("please stop")).unwrap();
    let followup = mailbox::TeammateMessage {
        from: TEAM_LEAD_NAME.to_string(),
        text: format!("please {FOLLOWUP}"),
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: None,
        summary: None,
    };
    mailbox::write_to_mailbox("worker", followup, "t").unwrap();

    let prompts = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let engine = RecordingEngine {
        prompts: prompts.clone(),
        // Reject the shutdown (do nothing on "shutdown request"); approve
        // only once the follow-up arrives, so the test terminates.
        approve_marker: FOLLOWUP.to_string(),
    };

    let result = run_in_process_teammate(in_process_config("initial"), &engine).await;

    let recorded = prompts.lock().await.clone();
    assert!(
        recorded.iter().any(|p| p.contains("shutdown request")),
        "the shutdown turn must have run; got: {recorded:?}"
    );
    assert!(
        recorded.iter().any(|p| p.contains(FOLLOWUP)),
        "rejecting shutdown must NOT terminate the loop — the follow-up \
         message must still be processed; got: {recorded:?}"
    );
    assert!(result.success, "error={:?}", result.error);
}
