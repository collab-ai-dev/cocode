//! Tests for `on_turn_interrupted_outcome` auto-restore decision matrix.
//!
//! Covers the `signal.reason === 'user-cancel'` path + idle guards +
//! `messagesAfterAreOnlySynthetic` predicate.
//!
//! The auto-restore decision is now centralised on this event —
//! removed from `on_turn_completed_outcome` and from
//! `update::exit::on_interrupt`.

use pretty_assertions::assert_eq;

use coco_types::ServerNotification;
use coco_types::SessionStartedParams;
use coco_types::TurnAbortReason;

use super::on_turn_interrupted_outcome;
use crate::state::AppState;
use crate::state::ModalState;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::SystemCellKind;
use crate::transcript::derive::test_helpers;

// ── Helpers ─────────────────────────────────────────────────────

fn user_cancel() -> TurnAbortReason {
    TurnAbortReason::UserCancel
}

fn system_preempt() -> TurnAbortReason {
    TurnAbortReason::SystemPreempt
}

// `TurnAbortReason` is now non-Option in `TurnOutcome::Interrupted` — the
// legacy "no reason" wire shape is gone. Tests that previously
// exercised the `None`-as-SystemPreempt fallback now exercise
// `system_preempt()` directly.
fn legacy_no_reason() -> TurnAbortReason {
    TurnAbortReason::SystemPreempt
}

/// Idle session with a single user message and a synthetic (empty)
/// assistant message — the "lossless tail" auto-restore scenario.
fn idle_with_lossless_tail(user_id: &str, user_text: &str) -> AppState {
    let mut s = AppState::new();
    test_helpers::push_user_text(&mut s.session, user_id, user_text);
    test_helpers::push_assistant_text(&mut s.session, "");
    s
}

/// Idle session with a user message followed by a real assistant
/// response — auto-restore must be suppressed.
fn idle_with_meaningful_tail() -> AppState {
    let mut s = AppState::new();
    test_helpers::push_user_text(&mut s.session, "u1", "ask");
    test_helpers::push_assistant_text(&mut s.session, "actual reply text");
    s
}

// ── Auto-restore matrix ─────────────────────────────────────────

/// Map a legacy test id ("u1") to the v5 UUID string the cell mirror
/// produces. `apply_auto_restore` reads message ids from
/// `transcript.cells()` (= `cell.message_uuid.to_string()`), so the
/// expected dispatched `Rewind { mode: AutoRestore }` carries the
/// same derivation, not the raw fixture id.
fn test_id(s: &str) -> String {
    crate::transcript::derive::id_to_uuid(s).to_string()
}

/// Channel pair scoped to one test. Caller drives `on_turn_interrupted`
/// with `&tx` and observes `rx.try_recv()` for the dispatched
/// `UserCommand::Rewind { mode: AutoRestore }`.
fn channel() -> (
    tokio::sync::mpsc::Sender<crate::command::UserCommand>,
    tokio::sync::mpsc::Receiver<crate::command::UserCommand>,
) {
    tokio::sync::mpsc::channel(16)
}

fn session_started(provider: &str) -> ServerNotification {
    ServerNotification::SessionStarted(SessionStartedParams {
        session_id: "s1".into(),
        protocol_version: "1.0".into(),
        cwd: "/tmp".into(),
        model: "model-a".into(),
        provider: provider.into(),
        permission_mode: "default".into(),
        tools: Vec::new(),
        slash_commands: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        plugins: Vec::new(),
        api_key_source: None,
        betas: Vec::new(),
        version: "0.0.1".into(),
        output_style: None,
        fast_mode_state: None,
        lsp_active: false,
    })
}

/// True if the receiver got a `Rewind { mode: AutoRestore }`. Drains
/// the channel; tests that need to inspect the message id should call
/// `rx.try_recv()` directly.
fn drained_auto_restore(
    rx: &mut tokio::sync::mpsc::Receiver<crate::command::UserCommand>,
) -> Option<String> {
    while let Ok(cmd) = rx.try_recv() {
        if let crate::command::UserCommand::Rewind {
            message_id,
            mode: crate::command::RewindMode::AutoRestore,
        } = cmd
        {
            return Some(message_id);
        }
    }
    None
}

#[test]
fn session_started_updates_provider_when_present() {
    let mut state = AppState::new();
    let (tx, _rx) = channel();

    super::handle(&mut state, session_started("openai"), &tx);

    assert_eq!(state.session.model, "model-a");
    assert_eq!(state.session.provider, "openai");
}

#[test]
fn session_started_preserves_provider_when_wire_field_is_absent() {
    let mut state = AppState::new();
    state.session.provider = "existing".into();
    let (tx, _rx) = channel();

    super::handle(&mut state, session_started(""), &tx);

    assert_eq!(state.session.provider, "existing");
}

#[test]
fn message_appended_projects_user_interruption_for_tool_use() {
    let mut state = AppState::new();
    let (tx, _rx) = channel();
    let message = coco_messages::create_user_interruption_system_message(true);
    let message_uuid = *message.uuid().expect("system interruption carries uuid");

    super::handle(
        &mut state,
        ServerNotification::MessageAppended {
            message: std::sync::Arc::new(message),
            session_id: String::new(),
            agent_id: None,
        },
        &tx,
    );

    let cells = state.session.transcript.cells();
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].message_uuid, message_uuid);
    let CellKind::System(SystemCellKind::UserInterruption { for_tool_use }) = cells[0].kind else {
        panic!("expected System(UserInterruption), got {:?}", cells[0].kind);
    };
    assert!(
        for_tool_use,
        "for_tool_use must remain engine-authoritative during projection"
    );
}

#[test]
fn user_cancel_with_lossless_tail_restores() {
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    let (tx, mut rx) = channel();
    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);

    // Auto-restore lives entirely on the engine round-trip — the TUI
    // dispatches `UserCommand::Rewind { mode: AutoRestore }` directly
    // and pulls the prompt back into the input; the actual transcript
    // truncation happens when `MessageTruncated` arrives from the
    // engine.
    assert_eq!(state.ui.input.text(), "original prompt");
    assert!(state.session.conversation_id.is_some());
    assert_eq!(
        drained_auto_restore(&mut rx).as_deref(),
        Some(test_id("u1").as_str()),
    );
}

#[test]
fn user_cancel_without_auto_restore_leaves_no_dispatch() {
    // Meaningful tail → no auto-restore → no Rewind dispatch.
    let mut state = idle_with_meaningful_tail();
    let (tx, mut rx) = channel();
    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);
    assert!(drained_auto_restore(&mut rx).is_none());
}

/// True when an auto-restore Rewind landed on the channel.
fn restored(rx: &mut tokio::sync::mpsc::Receiver<crate::command::UserCommand>) -> bool {
    drained_auto_restore(rx).is_some()
}

#[test]
fn user_cancel_with_meaningful_tail_does_not_restore() {
    let mut state = idle_with_meaningful_tail();
    let (tx, mut rx) = channel();
    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);

    // Auto-restore suppressed (meaningful tail). Engine pushes its
    // own `SystemMessage::UserInterruption` marker through
    // `MessageAppended` — tested at the renderer layer, not here.
    assert!(!restored(&mut rx));
    assert_eq!(state.ui.input.text(), "", "input unchanged");
}

#[test]
fn user_cancel_with_nonempty_input_does_not_restore() {
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    state.ui.input.textarea.set_text("user typed during cancel");
    let (tx, mut rx) = channel();

    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);

    // No restore: nonempty input gates it off.
    assert!(!restored(&mut rx));
    assert_eq!(
        state.ui.input.text(),
        "user typed during cancel",
        "user's in-flight text must NOT be clobbered",
    );
}

#[test]
fn user_cancel_with_active_surface_does_not_restore() {
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    state.ui.show_modal(ModalState::Help);
    let (tx, mut rx) = channel();

    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);

    assert!(!restored(&mut rx));
    assert_eq!(state.ui.input.text(), "");
}

#[test]
fn user_cancel_with_queued_command_does_not_restore() {
    use crate::state::QueuedCommandDisplay;
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    state
        .session
        .queued_commands
        .push_back(QueuedCommandDisplay {
            id: "q1".into(),
            preview: "next".into(),
            editable: true,
        });
    let (tx, mut rx) = channel();

    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);

    assert!(!restored(&mut rx));
    assert_eq!(state.ui.input.text(), "");
}

#[test]
fn system_preempt_never_restores() {
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    let (tx, mut rx) = channel();

    on_turn_interrupted_outcome(&mut state, system_preempt(), &tx);

    assert!(
        !restored(&mut rx),
        "Clear/Compact/Rewind/Shutdown drains must not auto-restore",
    );
    // SystemPreempt does NOT append the marker either — the
    // preempting op (Clear/Compact/Rewind/Shutdown) owns whatever
    // gets written next.
    assert_eq!(state.ui.input.text(), "");
}

#[test]
fn legacy_no_reason_is_treated_as_non_user_cancel() {
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    let (tx, mut rx) = channel();

    on_turn_interrupted_outcome(&mut state, legacy_no_reason(), &tx);

    assert!(!restored(&mut rx));
    assert_eq!(state.ui.input.text(), "");
}

#[test]
fn on_turn_interrupted_clears_streaming_and_busy() {
    let mut state = idle_with_lossless_tail("u1", "original prompt");
    state.ui.streaming = Some(crate::state::StreamingState::default());
    state.session.set_busy(true);
    let (tx, _rx) = channel();

    on_turn_interrupted_outcome(&mut state, user_cancel(), &tx);

    assert!(state.ui.streaming.is_none());
    assert!(!state.session.is_busy());
}

// ── TaskProgress.recent_activities → SubagentInstance copy ───────
//
// Wire-protocol regression coverage for commit e1e26559f7. The
// coordinator-side ring buffer is the source of truth; the TUI just
// copies the slice. Before the wire field existed, the TUI rebuilt
// its own ring from `last_tool_name` alone and dropped intermediate
// tools — this test pins the new "copy verbatim" behaviour.

fn running_subagent(agent_id: &str) -> crate::state::SubagentInstance {
    crate::state::SubagentInstance {
        kind: crate::state::session::SubagentKind::Subagent,
        agent_id: agent_id.into(),
        agent_type: "Explore".into(),
        description: "Find auth code".into(),
        status: crate::state::session::SubagentStatus::Running,
        color: None,
        team_name: None,
        started_at_ms: None,
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cost_usd: 0.0,
    }
}

fn progress_params(
    task_id: &str,
    last_tool: Option<&str>,
    activities: Vec<coco_types::TaskActivity>,
) -> coco_types::TaskProgressParams {
    coco_types::TaskProgressParams {
        task_id: task_id.into(),
        tool_use_id: None,
        description: "running".into(),
        usage: coco_types::TaskUsage {
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            tool_uses: activities.len() as i32,
            duration_ms: 1_000,
            cost_usd: 0.0,
        },
        last_tool_name: last_tool.map(str::to_string),
        summary: None,
        agent_type: None,
        recent_activities: activities,
        workflow_progress: Vec::new(),
    }
}

fn local_agent_started_params(task_id: &str, tool_use_id: &str) -> coco_types::TaskStartedParams {
    coco_types::TaskStartedParams {
        task_id: task_id.into(),
        tool_use_id: Some(tool_use_id.into()),
        description: "Map app root".into(),
        task_type: Some("local_agent".into()),
        workflow_name: None,
        prompt: None,
        agent_name: None,
        team_name: None,
        color: None,
        backend_kind: None,
    }
}

#[test]
fn task_started_resolves_subagent_type_from_tool_call_input() {
    // Regression: the row used to render the `local_agent` wire fallback until
    // the first `TaskProgress` carried the real type, so the badge visibly
    // flipped `local_agent → Explore` mid-run (staggered across siblings).
    // `ensure_subagent_row` now recovers the declared `subagent_type` from the
    // spawning Agent tool call's input, so the badge is correct on spawn.
    let mut state = AppState::new();
    let (tx, _rx) = channel();
    // The assistant turn that issued the Agent spawn must be in the transcript.
    test_helpers::push_tool_use_input(
        &mut state.session,
        "call-explore",
        coco_types::ToolName::Agent.as_str(),
        serde_json::json!({
            "description": "Map app root",
            "subagent_type": "Explore",
            "prompt": "explore",
        }),
    );

    let event = coco_types::ServerNotification::TaskStarted(local_agent_started_params(
        "agent-app-root",
        "call-explore",
    ));
    super::handle(&mut state, event, &tx);

    let agent = state
        .session
        .subagents
        .iter()
        .find(|a| a.agent_id == "agent-app-root")
        .expect("subagent row created");
    assert_eq!(agent.agent_type, "Explore");
}

#[test]
fn task_started_falls_back_to_wire_literal_when_tool_call_uncommitted() {
    // No issuing assistant turn in the transcript (the streaming race) → keep
    // the non-empty `local_agent` fallback; `TaskProgress` corrects it later.
    let mut state = AppState::new();
    let (tx, _rx) = channel();

    let event = coco_types::ServerNotification::TaskStarted(local_agent_started_params(
        "agent-x",
        "call-missing",
    ));
    super::handle(&mut state, event, &tx);

    let agent = state
        .session
        .subagents
        .iter()
        .find(|a| a.agent_id == "agent-x")
        .expect("subagent row created");
    assert_eq!(agent.agent_type, "local_agent");
}

#[test]
fn task_progress_copies_recent_activities_into_subagent() {
    let mut state = AppState::new();
    state.session.subagents.push(running_subagent("agent-7af2"));
    let (tx, _rx) = channel();

    let event = coco_types::ServerNotification::TaskProgress(progress_params(
        "agent-7af2",
        Some("Glob"),
        vec![
            coco_types::TaskActivity {
                tool_name: "Read".into(),
                summary: None,
            },
            coco_types::TaskActivity {
                tool_name: "Glob".into(),
                summary: None,
            },
        ],
    ));
    super::handle(&mut state, event, &tx);

    let agent = &state.session.subagents[0];
    let names: Vec<&str> = agent
        .recent_activities
        .iter()
        .map(|a| a.tool_name.as_str())
        .collect();
    assert_eq!(names, vec!["Read", "Glob"]);
    assert_eq!(agent.last_tool_name.as_deref(), Some("Glob"));
}

#[test]
fn task_progress_empty_recent_activities_leaves_prior_buffer_intact() {
    // A legacy producer that doesn't populate `recent_activities`
    // must not stomp the existing buffer to empty. Only non-empty
    // payloads replace state.
    let mut state = AppState::new();
    let mut agent = running_subagent("agent-1");
    agent.recent_activities = vec![coco_types::TaskActivity {
        tool_name: "Read".into(),
        summary: None,
    }];
    state.session.subagents.push(agent);
    let (tx, _rx) = channel();

    let event = coco_types::ServerNotification::TaskProgress(progress_params(
        "agent-1",
        Some("Bash"),
        vec![],
    ));
    super::handle(&mut state, event, &tx);

    let agent = &state.session.subagents[0];
    assert_eq!(agent.recent_activities.len(), 1);
    assert_eq!(agent.recent_activities[0].tool_name, "Read");
    assert_eq!(agent.last_tool_name.as_deref(), Some("Bash"));
}

#[test]
fn sandbox_violations_show_toast_not_modal() {
    let mut state = AppState::new();
    let (tx, _rx) = channel();

    super::handle(
        &mut state,
        coco_types::ServerNotification::SandboxViolationsDetected { count: 3 },
        &tx,
    );

    // Non-blocking count surface: a toast, not a per-burst blocking modal.
    assert_eq!(state.ui.toasts.len(), 1);
    assert!(
        state.ui.modal.is_none(),
        "violations must not open a blocking modal"
    );
}
