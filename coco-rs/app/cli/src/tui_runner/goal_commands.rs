pub(super) async fn run_goal_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    request: coco_commands::GoalCommandRequest,
) -> SlashFollowup {
    let runtime = session;
    let is_status = matches!(request, coco_commands::GoalCommandRequest::Status);
    let args = goal_command::goal_display_args(&request).to_string();
    let gate = goal_command::GoalGate {
        hooks_restricted: {
            let cfg = runtime.current_engine_config().await;
            cfg.disable_all_hooks || cfg.allow_managed_hooks_only
        },
        // Trust is required only interactively; the TUI is the interactive surface.
        trust_rejected: workspace_trust_rejected(),
    };
    let tokens_at_start = runtime.session_usage_snapshot().await.totals.output_tokens;
    let history_snapshot = runtime.history().lock().await.to_vec();
    let outcome = goal_command::resolve_goal_request(
        request,
        runtime.app_state(),
        &runtime.hook_registry(),
        &history_snapshot,
        tokens_at_start,
        gate,
    )
    .await;

    match outcome {
        goal_command::GoalOutcome::Text(text) => {
            if is_status {
                let (title, body) =
                    build_goal_status_modal(session, &history_snapshot, tokens_at_start, text)
                        .await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::OpenGoalStatus { title, body }))
                    .await;
            } else {
                emit_slash_text(event_tx, "goal", &args, &text).await;
            }
            SlashFollowup::Done
        }
        goal_command::GoalOutcome::StatusThenText { status, text } => {
            append_goal_status_and_slash_text(session, event_tx, status, &args, &text).await;
            emit_active_goal_snapshot(session, event_tx).await;
            SlashFollowup::Done
        }
        goal_command::GoalOutcome::SetAndRun {
            status,
            text,
            kickoff,
        } => {
            append_goal_status(session, event_tx, status).await;
            emit_active_goal_snapshot(session, event_tx).await;
            emit_slash_text(event_tx, "goal", &args, &text).await;
            SlashFollowup::RunEngine {
                content: kickoff,
                metadata: Some(format_slash_command_metadata("goal", &args)),
                thinking_level: None,
                model_runtime_source: None,
            }
        }
    }
}

pub(super) async fn build_goal_status_modal(
    session: &crate::session_runtime::SessionHandle,
    history: &[std::sync::Arc<coco_messages::Message>],
    current_output_tokens: i64,
    fallback_text: String,
) -> (String, String) {
    let runtime = session;
    if let Some(goal) = runtime.app_state().read().await.active_goal.clone() {
        return (
            "Goal active".to_string(),
            active_goal_modal_body(&goal, current_output_tokens),
        );
    }
    if let Some(goal) = goal_command::find_latest_goal_status(history)
        && goal.met
        && !goal.failed
        && !goal.sentinel
    {
        return ("Goal achieved".to_string(), achieved_goal_modal_body(&goal));
    }
    ("Goal".to_string(), fallback_text)
}

pub(super) fn active_goal_modal_body(
    goal: &coco_types::ActiveGoal,
    current_output_tokens: i64,
) -> String {
    let mut lines = vec![
        format!(
            "Running: {}",
            format_goal_duration_ms(goal_command::unix_time_ms().saturating_sub(goal.set_at_ms))
        ),
        format!(
            "Tokens: {}",
            current_output_tokens.saturating_sub(goal.tokens_at_start)
        ),
        format!("Iterations: {}", format_goal_iterations(goal.iterations)),
        String::new(),
        "Goal:".to_string(),
        goal.condition.clone(),
    ];
    if let Some(reason) = goal
        .last_reason
        .as_deref()
        .map(goal_command::format_goal_last_reason)
        .filter(|reason| !reason.is_empty())
    {
        lines.extend([String::new(), "Last check:".to_string(), reason]);
    }
    lines.extend([String::new(), "/goal clear to stop early".to_string()]);
    lines.join("\n")
}

pub(super) fn achieved_goal_modal_body(goal: &coco_types::GoalStatusPayload) -> String {
    let mut lines = Vec::new();
    let mut stats = Vec::new();
    if let Some(duration_ms) = goal.duration_ms {
        stats.push(format!("duration {}", format_goal_duration_ms(duration_ms)));
    }
    if let Some(iterations) = goal.iterations {
        stats.push(format!(
            "{} {}",
            iterations,
            if iterations == 1 { "turn" } else { "turns" }
        ));
    }
    if let Some(tokens) = goal.tokens {
        stats.push(format!("{} tokens", tokens.max(0)));
    }
    if !stats.is_empty() {
        lines.push(format!("Stats: {}", stats.join(" · ")));
        lines.push(String::new());
    }
    lines.push("Goal:".to_string());
    lines.push(goal.condition.clone());
    if let Some(reason) = goal
        .reason
        .as_deref()
        .map(goal_command::format_goal_last_reason)
        .filter(|reason| !reason.is_empty())
    {
        lines.extend([String::new(), "Reason:".to_string(), reason]);
    }
    lines.join("\n")
}

pub(super) fn format_goal_iterations(iterations: i32) -> String {
    if iterations <= 0 {
        "not yet evaluated".to_string()
    } else {
        format!(
            "{} {}",
            iterations,
            if iterations == 1 { "turn" } else { "turns" }
        )
    }
}

pub(super) fn format_goal_duration_ms(ms: i64) -> String {
    let seconds = (ms / 1000).max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{minutes}m")
        }
    }
}

pub(super) async fn append_goal_status(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
) {
    let runtime = session;
    let message = goal_status_message(payload);
    let mut history = runtime.history().lock().await;
    let event_tx_opt = Some(event_tx.clone());
    coco_query::history_sync::history_push_and_emit(&mut history, message, &event_tx_opt).await;
}

pub(super) async fn append_goal_status_and_slash_text(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) {
    let runtime = session;
    let mut messages = vec![goal_status_message(payload)];
    messages.extend(coco_messages::build_slash_command_messages(
        "goal", args, text, /*is_sensitive*/ false,
    ));
    {
        let mut history = runtime.history().lock().await;
        let event_tx_opt = Some(event_tx.clone());
        for message in messages.iter().cloned() {
            coco_query::history_sync::history_push_and_emit(&mut history, message, &event_tx_opt)
                .await;
        }
    }
    runtime.persist_local_transcript_messages(&messages).await;
}

pub(super) fn goal_status_message(
    payload: coco_types::GoalStatusPayload,
) -> coco_messages::Message {
    coco_messages::Message::Attachment(coco_messages::AttachmentMessage::silent_goal_status(
        payload,
    ))
}

pub(super) async fn emit_active_goal_snapshot(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    let goal = runtime.app_state().read().await.active_goal.clone();
    let _ = event_tx
        .send(CoreEvent::Protocol(
            goal_command::active_goal_changed_notification(goal.clone()),
        ))
        .await;
    runtime
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

pub(super) fn workspace_trust_rejected() -> bool {
    workspace_trust_rejected_from_env(
        std::env::var("COCO_WORKSPACE_TRUST_ACCEPTED")
            .ok()
            .as_deref(),
    )
}

pub(super) fn workspace_trust_rejected_from_env(value: Option<&str>) -> bool {
    matches!(value, Some("0"))
}

/// `/add-dir <path>` runner — validates and routes a session-scoped
/// `AddDirectories` update through local AppServer so the next batch's
/// permission context sees the wider scope. Source is `Session` — never
use coco_agent_host::goal_command;
use coco_query::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

use super::SlashFollowup;
use super::emit_slash_text;
use super::format_slash_command_metadata;
