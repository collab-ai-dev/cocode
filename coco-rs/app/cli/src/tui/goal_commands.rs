pub(super) async fn run_goal_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    request: coco_commands::GoalCommandRequest,
) -> SlashFollowup {
    let is_status = matches!(request, coco_commands::GoalCommandRequest::Status);
    let args = goal_command::goal_display_args(&request).to_string();
    // Trust is required only interactively; the TUI is the interactive surface.
    let outcome = goal_command::resolve_goal_request_for_session(
        session,
        request,
        workspace_trust_rejected(),
    )
    .await;

    match outcome {
        goal_command::GoalOutcome::Text(text) => {
            if is_status {
                let modal = goal_command::build_goal_status_modal_for_session(session, text).await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::OpenGoalStatus {
                        title: modal.title,
                        body: modal.body,
                    }))
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
                metadata: Some(coco_agent_host::session_messages::slash_command_metadata(
                    "goal", &args,
                )),
                thinking_level: None,
                model_runtime_source: None,
            }
        }
    }
}

pub(super) async fn append_goal_status(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
) {
    let messages = goal_command::append_goal_status_to_history(session, payload).await;
    emit_appended_messages(event_tx, messages).await;
}

pub(super) async fn append_goal_status_and_slash_text(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) {
    let messages =
        goal_command::append_goal_status_and_slash_to_history(session, payload, args, text).await;
    emit_appended_messages(event_tx, messages).await;
}

pub(super) async fn emit_active_goal_snapshot(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let goal = goal_command::persist_active_goal_snapshot(session).await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            goal_command::active_goal_changed_notification(goal.clone()),
        ))
        .await;
}

async fn emit_appended_messages(
    event_tx: &mpsc::Sender<CoreEvent>,
    messages: Vec<std::sync::Arc<coco_messages::Message>>,
) {
    for message in messages {
        let _ = event_tx
            .send(CoreEvent::Protocol(
                coco_types::ServerNotification::MessageAppended {
                    message,
                    identity: coco_types::ServerNotificationIdentity::default(),
                },
            ))
            .await;
    }
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
