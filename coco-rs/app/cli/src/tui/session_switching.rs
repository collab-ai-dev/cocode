pub(super) async fn emit_resume_plan_ui_state(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    restored_v1_todos: Option<Vec<coco_types::TodoRecord>>,
) {
    let prior_messages = plan
        .prior_messages
        .iter()
        .cloned()
        .map(std::sync::Arc::new)
        .collect::<Vec<_>>();
    // Goal state is recovered by the first-class goal runtime (restored from the
    // durable `GoalSnapshot` during session build); read its current snapshot view.
    let goal = goal_command::read_goal_snapshot_view(session).await;

    // Bulk resume hydration mirrors the startup `--resume` path:
    // reset UI-only state first, then replace transcript scrollback in
    // one pass instead of replaying thousands of individual appends.
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::SessionResetForResume {
                identity: coco_types::ServerNotificationIdentity::new(
                    Some(plan.session_id.clone()),
                    None,
                ),
            },
        ))
        .await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::HistoryReplaced {
                messages: prior_messages.clone(),
                identity: coco_types::ServerNotificationIdentity::new(
                    Some(plan.session_id.clone()),
                    None,
                ),
                reason: coco_types::HistoryReplaceReason::Hydrate,
            },
        ))
        .await;
    if let Some(todos) = restored_v1_todos {
        let mut todos_by_agent = HashMap::new();
        if !todos.is_empty() {
            todos_by_agent.insert(plan.session_id.to_string(), todos);
        }
        let _ = event_tx
            .send(CoreEvent::Protocol(
                coco_types::ServerNotification::TaskPanelChanged(
                    coco_types::TaskPanelChangedParams {
                        plan_tasks: Vec::new(),
                        todos_by_agent,
                        expanded_view: coco_types::ExpandedView::None,
                        verification_nudge_pending: false,
                        // Unordered producer: always applied, never
                        // advances the consumer's high-water mark.
                        generation: 0,
                    },
                ),
            ))
            .await;
    }
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::SessionUsageUpdated(Box::new(
                session.session_usage_snapshot().await,
            )),
        ))
        .await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            goal_command::goal_snapshot_changed_notification(goal.clone()),
        ))
        .await;
}

pub(super) async fn emit_resume_plan_ui_state_for_runtime(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let restored_v1_todos =
        if coco_agent_host::session_controls::should_restore_v1_todos_on_resume(session) {
            latest_todo_write_todos(&plan.prior_messages)
        } else {
            None
        };
    if let Some(todos) = restored_v1_todos.clone() {
        session
            .seed_todo_list_snapshot(plan.session_id.to_string(), todos)
            .await;
    }
    emit_resume_plan_ui_state(plan, session, event_tx, restored_v1_todos).await;
}

#[derive(serde::Deserialize)]
pub(super) struct TodoWriteTranscriptInput {
    todos: Vec<coco_types::TodoRecord>,
}

pub(super) fn todo_write_store_snapshot(
    todos: Vec<coco_types::TodoRecord>,
) -> Vec<coco_types::TodoRecord> {
    if !todos.is_empty() && todos.iter().all(|todo| todo.status == "completed") {
        Vec::new()
    } else {
        todos
    }
}

pub(super) fn latest_todo_write_todos(messages: &[Message]) -> Option<Vec<coco_types::TodoRecord>> {
    for message in messages.iter().rev() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            continue;
        };
        for part in content.iter().rev() {
            let AssistantContent::ToolCall(call) = part else {
                continue;
            };
            if call.tool_name != coco_types::ToolName::TodoWrite.as_str() {
                continue;
            }
            match serde_json::from_value::<TodoWriteTranscriptInput>(call.input.clone()) {
                Ok(input) => return Some(todo_write_store_snapshot(input.todos)),
                Err(err) => {
                    warn!(error = %err, "failed to restore TodoWrite state from transcript input");
                    return None;
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_resume(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    let target = args.trim();
    if target.is_empty() {
        let sessions = match runtime.list_persisted_session_summaries().await {
            Ok(result) => result.sessions,
            Err(err) => {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    "resume",
                    args,
                    &format!("Failed to list sessions: {err}"),
                )
                .await;
                return SlashOutcome::Handled;
            }
        };
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenSessionBrowser {
                sessions,
            }))
            .await;
        return SlashOutcome::Handled;
    }

    match coco_agent_host::runtime_resume::load_resume_plan_for_runtime_target(session, target)
        .await
    {
        Ok(plan) => {
            tracing::info!(
                target: "coco_agent_host::resume",
                session_id = %plan.session_id,
                source_session_id = %plan.source_session_id,
                prior_messages = plan.prior_messages.len(),
                "slash resume: hydrating session",
            );
            if !switch_to_resume_plan_through_app_server(
                &plan,
                "resume",
                args,
                current_session,
                event_tx,
                local_app_server_bridge,
                runtime_reload_subscriptions,
                session.session_id(),
            )
            .await
            {
                return SlashOutcome::Handled;
            }
            // Reconcile coordinator mode to the resumed session. Runs at a
            // turn boundary, so the env flip is
            // observed by the next prompt assembly.
            if let Some(warning) =
                runtime.reconcile_session_mode_on_resume(plan.conversation.mode.as_deref())
            {
                emit_slash_text(event_tx, &plan.session_id, "resume", args, warning).await;
            }
        }
        Err(err) => {
            emit_slash_text(
                event_tx,
                session.session_id(),
                "resume",
                args,
                &format!("Failed to resume session: {err}"),
            )
            .await;
        }
    }

    SlashOutcome::Handled
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn switch_to_resume_plan_through_app_server(
    plan: &ResumePlan,
    command_name: &str,
    args: &str,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    output_session_id: &coco_types::SessionId,
) -> bool {
    match apply_resume_plan_through_app_server(
        plan,
        current_session,
        event_tx,
        local_app_server_bridge,
        runtime_reload_subscriptions,
    )
    .await
    {
        Ok(()) => true,
        Err(err) => {
            emit_slash_text(
                event_tx,
                output_session_id,
                command_name,
                args,
                &format!("Failed to resume session: {err}"),
            )
            .await;
            false
        }
    }
}

pub(super) async fn apply_resume_plan_through_app_server(
    plan: &ResumePlan,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> anyhow::Result<()> {
    let binding = local_app_server_bridge
        .replace_interactive_session_with_resume(
            coco_types::SessionResumeParams {
                target: coco_types::SessionTarget {
                    session_id: plan.session_id.clone(),
                },
                plan_mode_instructions: None,
            },
            Some(event_tx.clone()),
        )
        .await?;
    let new_session = binding.session;

    {
        let mut current = current_session.write().await;
        *current = new_session.clone();
    }
    runtime_reload_subscriptions
        .lock()
        .await
        .install_for_session(&new_session)
        .await;
    emit_resume_plan_ui_state_for_runtime(plan, &new_session, event_tx).await;
    Ok(())
}

/// `/branch` (alias `/fork`) — fork the current conversation at this point
/// into a NEW session and switch to it live.
/// Delegates transcript forking to the runtime resume layer, then hydrates the
/// runtime onto the fork through local AppServer `session/resume`. The original
/// session is left untouched on disk.
#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_branch(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    let custom_title = args.trim().to_string();
    match coco_agent_host::runtime_resume::fork_resume_plan_for_runtime_session(runtime).await {
        Ok(plan) => {
            let new_id = plan.session_id.to_string();
            let source_id = plan.source_session_id.to_string();
            // Derive the fork title: explicit arg, else the first user prompt
            // (truncated), suffixed "(Branch)" — branch.ts. Done
            // BEFORE hydrate moves `plan`.
            let base_title = if custom_title.is_empty() {
                first_user_prompt_title(&plan.prior_messages)
            } else {
                Some(custom_title)
            };
            if !switch_to_resume_plan_through_app_server(
                &plan,
                "branch",
                args,
                current_session,
                event_tx,
                local_app_server_bridge,
                runtime_reload_subscriptions,
                session.session_id(),
            )
            .await
            {
                return SlashOutcome::Handled;
            }
            // Reconcile coordinator mode onto the fork, same as /resume — the
            // fork inherits the source's persisted mode. Runs at a turn
            // boundary so the next prompt assembly observes the flip.
            if let Some(warning) =
                runtime.reconcile_session_mode_on_resume(plan.conversation.mode.as_deref())
            {
                emit_slash_text(event_tx, &plan.session_id, "branch", args, warning).await;
            }
            // The fork is now the live session, so session/rename titles it.
            if let Some(base) = base_title {
                let title = format!("{base} (Branch)");
                if let Err(e) = local_app_server_bridge
                    .client()
                    .session_rename(
                        local_app_server_bridge.handler(),
                        coco_types::SessionRenameParams {
                            target: coco_types::SessionTarget {
                                session_id: plan.session_id.clone(),
                            },
                            name: title,
                        },
                    )
                    .await
                {
                    warn!(error = %e, "failed to set /branch fork title");
                }
            }
            emit_slash_text(
                event_tx,
                &plan.session_id,
                "branch",
                args,
                &format!(
                    "Branched into a new session ({new_id}). \
                     To return to the original, /resume {source_id}."
                ),
            )
            .await;
        }
        Err(err) => {
            emit_slash_text(
                event_tx,
                session.session_id(),
                "branch",
                "",
                &format!("Failed to branch: {err}"),
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// Derive a short title from the first user message's text (first line,
/// truncated), for naming a `/branch` fork when no explicit title is given.
pub(super) fn first_user_prompt_title(messages: &[coco_messages::Message]) -> Option<String> {
    let text = messages.iter().find_map(|m| {
        matches!(m, coco_messages::Message::User(_))
            .then(|| coco_messages::wrapping::extract_text_from_message(m))
            .filter(|t| !t.trim().is_empty())
    })?;
    let first_line = text.trim().lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }
    let truncated: String = first_line.chars().take(40).collect();
    Some(truncated)
}

#[cfg(test)]
pub(super) fn session_plan_file_path(
    config_home: &std::path::Path,
    project_dir: Option<&std::path::Path>,
    plans_directory_setting: Option<&str>,
    session_id: &coco_types::SessionId,
) -> std::path::PathBuf {
    let plans_dir =
        coco_context::resolve_plans_directory(config_home, project_dir, plans_directory_setting);
    coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None)
}

pub(super) fn runtime_session_plan_file_path(
    session: &crate::session_runtime::SessionHandle,
) -> std::path::PathBuf {
    session.session_plan_file_path()
}
use std::{collections::HashMap, sync::Arc};

use coco_agent_host::{goal_command, resume_resolver::ResumePlan};
use coco_messages::{AssistantContent, LlmMessage, Message};
use coco_query::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::{Mutex, mpsc};
use tracing::warn;

use super::{SharedSessionHandle, SlashOutcome, TuiRuntimeReloadSubscriptions, emit_slash_text};
