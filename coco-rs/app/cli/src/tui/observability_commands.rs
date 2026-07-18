pub(super) async fn run_file_history_diff_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    args: &str,
) {
    let mut parts = args.split_whitespace();
    match parts.next() {
        Some("session") => {
            let text = match session_controls::file_history_diff(
                Some(session.clone()),
                FileHistoryDiffTarget::Session,
            )
            .await
            {
                Ok(diff) => format_file_history_diff("Session diff", diff),
                Err(error) => format_file_history_diff_error(error),
            };
            emit_slash_text(event_tx, session.session_id(), "diff", args, &text).await;
        }
        Some("turn") => {
            let Some(message_id) = parts.next() else {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    "diff",
                    args,
                    "Usage: /diff turn <message-id>",
                )
                .await;
                return;
            };
            let text = match session_controls::file_history_diff(
                Some(session.clone()),
                FileHistoryDiffTarget::Turn { message_id },
            )
            .await
            {
                Ok(diff) => format_file_history_diff("Turn diff", diff),
                Err(error) => format_file_history_diff_error(error),
            };
            emit_slash_text(event_tx, session.session_id(), "diff", args, &text).await;
        }
        _ => {
            emit_slash_text(
                event_tx,
                session.session_id(),
                "diff",
                args,
                "Usage: /diff session | /diff turn <message-id>",
            )
            .await;
        }
    }
}

pub(super) fn format_file_history_diff_error(error: SessionControlError) -> String {
    match error {
        SessionControlError::FileDiffNotEnabled => {
            "File history is not enabled for this session.".to_string()
        }
        SessionControlError::FileDiffSnapshotMissing(_) => {
            "No snapshot found for message id.".to_string()
        }
        SessionControlError::FileDiffOperation { context, source } => {
            format!("Unable to build {context}: {source}")
        }
        error => error.to_string(),
    }
}

pub(super) fn format_file_history_diff(title: &str, diff: coco_context::RenderedDiff) -> String {
    if diff.stats.files_changed.is_empty() {
        return format!("{title}: no file-history changes.");
    }

    let mut out = format!(
        "{title}: {} file{}, +{}, -{}\n\n",
        diff.stats.files_changed.len(),
        if diff.stats.files_changed.len() == 1 {
            ""
        } else {
            "s"
        },
        diff.stats.insertions,
        diff.stats.deletions
    );
    append_truncated_file_history_diff(&mut out, &diff.unified_diff);
    out
}

pub(super) fn append_truncated_file_history_diff(out: &mut String, diff: &str) {
    let trimmed = diff.trim();
    if trimmed.len() <= MAX_FILE_HISTORY_DIFF_CHARS {
        out.push_str(trimmed);
        return;
    }

    let head = coco_utils_string::take_bytes_at_char_boundary(trimmed, MAX_FILE_HISTORY_DIFF_CHARS);
    let truncate_at = head.rfind('\n').unwrap_or(head.len());
    out.push_str(&trimmed[..truncate_at]);
    let remaining_lines = trimmed[truncate_at..].lines().count();
    out.push_str(&format!("\n\n... truncated ({remaining_lines} more lines)"));
}

pub(super) async fn run_tasks_command(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    name: &str,
    args: &str,
) {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenBackgroundTasks))
            .await;
        return;
    }

    let mut parts = trimmed.split_whitespace();
    match parts.next() {
        Some("list") => {
            if let Err(error) = local_app_server_bridge
                .activate_existing_interactive_session(session.session_id().clone(), None)
            {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    &format!("Failed to activate AppServer session: {error}"),
                )
                .await;
                return;
            }
            match local_app_server_bridge
                .client()
                .task_list(
                    local_app_server_bridge.handler(),
                    interactive_session(local_app_server_bridge),
                )
                .await
            {
                Ok(result) => {
                    let text = format_task_list(&result.tasks);
                    emit_slash_text(event_tx, session.session_id(), name, args, &text).await;
                }
                Err(err) => {
                    emit_slash_text(
                        event_tx,
                        session.session_id(),
                        name,
                        args,
                        &format!("Failed to list tasks: {err}"),
                    )
                    .await;
                }
            }
        }
        Some("detail") => {
            let Some(task_id) = parts.next() else {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    "Usage: /tasks detail <id>",
                )
                .await;
                return;
            };
            if let Err(error) = local_app_server_bridge
                .activate_existing_interactive_session(session.session_id().clone(), None)
            {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    &format!("Failed to activate AppServer session: {error}"),
                )
                .await;
                return;
            }
            match local_app_server_bridge
                .client()
                .task_detail(
                    local_app_server_bridge.handler(),
                    coco_types::TaskDetailParams {
                        target: session_target(local_app_server_bridge),
                        task_id: task_id.to_string(),
                    },
                )
                .await
            {
                Ok(result) => {
                    let text = format_task_detail(&result);
                    emit_slash_text(event_tx, session.session_id(), name, args, &text).await;
                }
                Err(err) => {
                    emit_slash_text(
                        event_tx,
                        session.session_id(),
                        name,
                        args,
                        &format!("Failed to read task: {err}"),
                    )
                    .await;
                }
            }
        }
        Some("cancel") => {
            let Some(task_id) = parts.next() else {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    "Usage: /tasks cancel <id>",
                )
                .await;
                return;
            };
            if let Err(error) = local_app_server_bridge
                .activate_existing_interactive_session(session.session_id().clone(), None)
            {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    &format!("Failed to activate AppServer session: {error}"),
                )
                .await;
                return;
            }
            match local_app_server_bridge
                .client()
                .stop_task(
                    local_app_server_bridge.handler(),
                    coco_types::StopTaskParams {
                        target: interactive_target(local_app_server_bridge),
                        task_id: task_id.to_string(),
                    },
                )
                .await
            {
                Ok(()) => {
                    emit_slash_text(
                        event_tx,
                        session.session_id(),
                        name,
                        args,
                        &format!("Cancelled task {task_id}."),
                    )
                    .await;
                }
                Err(err) => {
                    emit_slash_text(
                        event_tx,
                        session.session_id(),
                        name,
                        args,
                        &format!("Failed to cancel task {task_id}: {err}"),
                    )
                    .await;
                }
            }
        }
        Some(_) | None => {
            emit_slash_text(
                event_tx,
                session.session_id(),
                name,
                args,
                "Usage: /tasks [list|detail <id>|cancel <id>]",
            )
            .await;
        }
    }
}

pub(super) async fn toggle_fast_mode_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    let active = match session_controls::next_fast_mode_state(Some(session.clone())).await {
        Ok(active) => active,
        Err(error) => {
            warn!(%error, "TUI ToggleFastMode could not resolve next fast mode state");
            return;
        }
    };
    if let Err(error) = local_app_server_bridge
        .activate_existing_interactive_session(session.session_id().clone(), Some(event_tx.clone()))
    {
        warn!(%error, "TUI ToggleFastMode could not activate local AppServer session");
        return;
    }

    let mut settings = HashMap::new();
    settings.insert("fast_mode".to_string(), serde_json::json!(active));
    if let Err(error) = local_app_server_bridge
        .client()
        .config_apply_flags(
            local_app_server_bridge.handler(),
            coco_types::ConfigApplyFlagsParams {
                target: interactive_target(local_app_server_bridge),
                settings,
            },
        )
        .await
    {
        warn!(%error, "TUI ToggleFastMode via AppServerLocalBridge failed");
    }
}

pub(super) async fn set_thinking_level_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    level: String,
) {
    let effort = match level.parse::<coco_types::ReasoningEffort>() {
        Ok(effort) => effort,
        Err(err) => {
            tracing::warn!(level = %level, error = %err, "SetThinkingLevel: bad effort string, ignoring");
            return;
        }
    };
    if let Err(error) = local_app_server_bridge
        .activate_existing_interactive_session(session.session_id().clone(), Some(event_tx.clone()))
    {
        warn!(%error, "TUI SetThinkingLevel could not activate local AppServer session");
        return;
    }

    if let Err(error) = local_app_server_bridge
        .client()
        .set_thinking(
            local_app_server_bridge.handler(),
            coco_types::SetThinkingParams {
                target: interactive_target(local_app_server_bridge),
                thinking_level: Some(coco_types::ThinkingLevel {
                    effort,
                    budget_tokens: None,
                    options: HashMap::new(),
                }),
            },
        )
        .await
    {
        warn!(%error, "TUI SetThinkingLevel via AppServerLocalBridge failed");
    }
}

pub(super) fn format_task_list(tasks: &[coco_types::TaskStateBase]) -> String {
    if tasks.is_empty() {
        return "No tasks in this session.".to_string();
    }

    let mut out = String::from("Active tasks:\n\n");
    for task in tasks {
        out.push_str(&format!(
            "- {}  {:?}  {}\n",
            task.id, task.status, task.description
        ));
    }
    out
}

pub(super) fn format_task_detail(result: &coco_types::TaskDetailResult) -> String {
    let mut out = format!("Task {}\n\n", result.task_id);
    out.push_str(&format!("Interrupted: {}\n", result.interrupted));
    if let Some(code) = result.exit_code {
        out.push_str(&format!("Exit code: {code}\n"));
    }
    if !result.stdout.trim().is_empty() {
        out.push_str("\nstdout:\n");
        out.push_str(&result.stdout);
        if !result.stdout.ends_with('\n') {
            out.push('\n');
        }
    }
    if !result.stderr.trim().is_empty() {
        out.push_str("\nstderr:\n");
        out.push_str(&result.stderr);
        if !result.stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Pure decision used by `dispatch_plan`: after a `/plan <description>`
/// successfully flips into plan mode, should the slash command fire a
/// query for the description? Returns `Some (trimmed_description)` when a
/// query should fire (`description` is non-empty and not `"open"`), else
/// `None`. Pure so this rule is regression-tested without a
/// `SessionRuntime` fixture.
pub(super) fn plan_command_query_after_flip(args: &str) -> Option<&str> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "open" {
        None
    } else {
        Some(trimmed)
    }
}

/// `/plan` dispatch with full session-runtime context.
/// Typing `/plan` IS the consent to enter plan mode, so the dispatcher
/// flips state directly via the same dual-write path
/// `UserCommand::SetPermissionMode` uses (engine_config + app_state)
/// plus the plan-mode-specific patch (`pre_plan_mode`,
/// `plan_mode_entry_ms`, `needs_plan_mode_exit_attachment` cleared).
/// The model never sees a redundant `EnterPlanMode` Yes/No dialog.
/// Per-arg behaviour:
/// - `""` → flip if needed, then show current plan or hint
/// - `"open"` → flip if needed, ensure file, launch `$EDITOR`/`vi`
/// - `<description>` → flip if needed; if state changed, fire a query
/// with the description; if already in plan mode, ignore the
use std::collections::HashMap;

use coco_agent_host::session_controls::{self, FileHistoryDiffTarget, SessionControlError};
use coco_query::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;
use tracing::warn;

use super::{
    MAX_FILE_HISTORY_DIFF_CHARS, emit_slash_text, interactive_session, interactive_target,
    session_target,
};
