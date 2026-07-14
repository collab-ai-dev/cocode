/// Create a selected `/memory` target if needed and launch the configured
/// editor. Effects live in the CLI bridge so TUI reducers stay pure.
pub(super) async fn run_open_memory_file(
    path: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let path_display = path.display().to_string();
    let result = tokio::task::spawn_blocking(move || open_memory_file_blocking(&path)).await;

    let event = match result {
        Ok(Ok(())) => TuiOnlyEvent::MemoryFileOpened { path: path_display },
        Ok(Err(error)) => TuiOnlyEvent::MemoryFileOpenFailed {
            path: path_display,
            error,
        },
        Err(err) => {
            warn!(error = %err, "memory editor task panicked");
            TuiOnlyEvent::MemoryFileOpenFailed {
                path: path_display,
                error: format!("memory editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

/// Create this session's plan target if needed and launch the configured
/// editor. Uses the same terminal handoff as prompt and memory editing.
pub(super) async fn run_open_plan_file(
    path: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let path_display = path.display().to_string();
    let result = tokio::task::spawn_blocking(move || open_plan_file_blocking(&path)).await;

    let event = match result {
        Ok(Ok(())) => TuiOnlyEvent::PlanFileOpened { path: path_display },
        Ok(Err(error)) => TuiOnlyEvent::PlanFileOpenFailed {
            path: path_display,
            error,
        },
        Err(err) => {
            warn!(error = %err, "plan editor task panicked");
            TuiOnlyEvent::PlanFileOpenFailed {
                path: path_display,
                error: format!("plan editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

pub(super) async fn run_plan_prompt_editor(
    request_id: String,
    initial_content: String,
    path: Option<std::path::PathBuf>,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let event_request_id = request_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        open_plan_prompt_editor_blocking(&initial_content, path.as_deref())
    })
    .await;

    let event = match result {
        Ok(Ok((content, modified))) => TuiOnlyEvent::ExitPlanPromptEditorCompleted {
            request_id: event_request_id,
            content,
            modified,
        },
        Ok(Err(error)) => TuiOnlyEvent::ExitPlanPromptEditorFailed {
            request_id: event_request_id,
            error,
        },
        Err(err) => {
            warn!(error = %err, "exit-plan prompt editor task panicked");
            TuiOnlyEvent::ExitPlanPromptEditorFailed {
                request_id: event_request_id,
                error: format!("plan editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

pub(super) async fn emit_editor_prepare_failed(
    request: PendingEditorRequest,
    error: String,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let message = format!("failed to prepare terminal for editor: {error}");
    let event = match request {
        PendingEditorRequest::Memory { path } => TuiOnlyEvent::MemoryFileOpenFailed {
            path: path.display().to_string(),
            error: message,
        },
        PendingEditorRequest::Plan { path } => TuiOnlyEvent::PlanFileOpenFailed {
            path: path.display().to_string(),
            error: message,
        },
        PendingEditorRequest::PlanPrompt { request_id, .. } => {
            TuiOnlyEvent::ExitPlanPromptEditorFailed {
                request_id,
                error: message,
            }
        }
        PendingEditorRequest::Prompt { .. } => TuiOnlyEvent::PromptEditorFailed { error: message },
        // Agent editor preparation failure is surfaced via the
        // generic prompt-editor channel (no dedicated wire event).
        // The user still sees a toast and the dialog stays mounted.
        PendingEditorRequest::Agent { .. } => TuiOnlyEvent::PromptEditorFailed { error: message },
    };
    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

/// Fork `$EDITOR` against the agent markdown file. On clean exit the runner
/// asks the host to reload the agent catalog **only when the file actually
/// changed** so an editor session that quit without saving doesn't churn the
/// catalog. Falls back to reload on any mtime-read error so a missing-stat
/// doesn't strand the dialog.
pub(super) async fn run_open_agent_file(
    session: crate::session_runtime::SessionHandle,
    path: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) {
    let path_display = path.display().to_string();
    let mtime_before = file_mtime(&path);
    let editor_path = path.clone();
    let result = tokio::task::spawn_blocking(move || run_editor_on_file(&editor_path)).await;

    match result {
        Ok(Ok(())) => {
            let mtime_after = file_mtime(&path);
            // Skip the reload when mtime is known on both sides and
            // unchanged — common case for "opened, looked, quit
            // without writing". Either side missing falls back to
            // reload so a transient stat() failure doesn't desync
            // the dialog.
            let unchanged = matches!((mtime_before, mtime_after), (Some(a), Some(b)) if a == b);
            if unchanged {
                tracing::debug!(
                    target: "coco::agents",
                    %path_display,
                    "agent editor exited with no file changes; skipping reload"
                );
                refresh_agents_dialog(&session, &event_tx).await;
                return;
            }
            // Reload + republish the dialog payload so the user sees
            // their edits immediately. Live registry refresh + dialog
            // refresh both go through the existing wire so observers
            // (subagent dispatch, dialog renderer) stay coherent.
            coco_agent_host::session_agents::reload_agent_catalog(&session).await;
            refresh_agents_dialog(&session, &event_tx).await;
        }
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco::agents",
                %path_display,
                %error,
                "agent editor failed"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "coco::agents",
                %path_display,
                error = %err,
                "agent editor task panicked"
            );
        }
    }
}

/// Read the file's modification time, dropping any error to `None`.
/// Used as a cheap change-detection signal for the post-edit reload
/// short-circuit; any stat hiccup falls back to the safe "reload"
/// path so we never serve a stale dialog.
pub(super) fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Push a fresh host-built agents dialog payload to the TUI. Used after CRUD
/// (`OpenAgentEditor` exit, `DeleteAgentFile`) so the dialog refreshes in place
/// rather than waiting for the user to re-issue `/agents`.
pub(super) async fn refresh_agents_dialog(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let payload = coco_agent_host::session_dialogs::build_agents_dialog_payload(session).await;
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenAgentsDialog { payload }))
        .await;
}

/// Re-emit `OpenPermissionsEditor` with a fresh snapshot so the open
/// overlay refreshes in place after a persisted edit.
pub(super) async fn refresh_permissions_editor(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let payload = coco_agent_host::session_dialogs::build_permissions_editor_payload(session).await;
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenPermissionsEditor {
            payload,
        }))
        .await;
}

/// Apply one `/permissions`-editor update to the live `ToolAppState.permissions`
/// base and persist it to its destination settings file. Mirrors the
/// `ApprovalResponse` "Always Allow" apply+persist path, but the editor
/// targets any of the three writable scopes (User / Project / Local).
/// Routes through local AppServer `control/applyPermissionUpdate`; the SDK
/// handler folds the update into the live base (via
/// `apply_permission_updates_to_live`) AND persists persistable destinations
/// to disk.
pub(super) async fn apply_and_persist_permission_update(
    update: &coco_types::PermissionUpdate,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .apply_permission_update(
            local_app_server_bridge.handler(),
            coco_types::ApplyPermissionUpdateParams {
                target: interactive_target(local_app_server_bridge),
                update: update.clone(),
            },
        )
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::Error(
                coco_types::ErrorParams {
                    message: format!("failed to apply permission update: {error}"),
                    category: Some("permission_update_failed".to_string()),
                    retryable: true,
                },
            )))
            .await;
        return false;
    }
    true
}

/// Clear session-scoped permission rules through local AppServer runtime control.
pub(super) async fn reset_session_permission_rules(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .reset_session_permission_rules(
            local_app_server_bridge.handler(),
            interactive_session(local_app_server_bridge),
        )
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::Error(
                coco_types::ErrorParams {
                    message: format!("failed to reset session permission rules: {error}"),
                    category: Some("permission_reset_failed".to_string()),
                    retryable: true,
                },
            )))
            .await;
        return false;
    }
    true
}

pub(super) async fn set_agent_color(
    color: Option<coco_types::AgentColorName>,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .set_agent_color(
            local_app_server_bridge.handler(),
            coco_types::SetAgentColorParams {
                target: interactive_target(local_app_server_bridge),
                color,
            },
        )
        .await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::Error(
                coco_types::ErrorParams {
                    message: format!("failed to set session color: {error}"),
                    category: Some("agent_color_failed".to_string()),
                    retryable: true,
                },
            )))
            .await;
        return false;
    }
    true
}

pub(super) fn open_memory_file_blocking(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create parent directory: {err}"))?;
    }

    // `wx` semantics: create exclusively, but an existing memory file is
    // fine. We just need the target present before launching the editor.
    if let Err(err) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        && err.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(format!("failed to create memory file: {err}"));
    }

    run_editor_on_file(path)
}

pub(super) fn open_plan_file_blocking(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create plans directory: {err}"))?;
    }

    if let Err(err) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        && err.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(format!("failed to create plan file: {err}"));
    }

    run_editor_on_file(path)
}

pub(super) async fn run_prompt_editor(initial_content: String, event_tx: mpsc::Sender<CoreEvent>) {
    let result =
        tokio::task::spawn_blocking(move || open_prompt_editor_blocking(&initial_content)).await;

    let event = match result {
        Ok(Ok((content, modified))) => TuiOnlyEvent::PromptEditorCompleted { content, modified },
        Ok(Err(error)) => TuiOnlyEvent::PromptEditorFailed { error },
        Err(err) => {
            warn!(error = %err, "prompt editor task panicked");
            TuiOnlyEvent::PromptEditorFailed {
                error: format!("prompt editor task failed: {err}"),
            }
        }
    };

    let _ = event_tx.send(CoreEvent::Tui(event)).await;
}

pub(super) fn open_prompt_editor_blocking(initial_content: &str) -> Result<(String, bool), String> {
    let path = std::env::temp_dir().join(format!("coco-prompt-edit-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&path, initial_content)
        .map_err(|err| format!("failed to write editor temp file: {err}"))?;

    let result = run_editor_on_file(&path).and_then(|()| {
        let content = std::fs::read_to_string(&path)
            .map_err(|err| format!("failed to read editor temp file: {err}"))?;
        let modified = content != initial_content;
        Ok((content, modified))
    });

    if let Err(err) = std::fs::remove_file(&path)
        && result.is_ok()
    {
        return Err(format!("failed to remove editor temp file: {err}"));
    }

    result
}

pub(super) fn open_plan_prompt_editor_blocking(
    initial_content: &str,
    path: Option<&std::path::Path>,
) -> Result<(String, bool), String> {
    let Some(path) = path else {
        return open_prompt_editor_blocking(initial_content);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create plans directory: {err}"))?;
    }
    if !path.exists() {
        std::fs::write(path, initial_content)
            .map_err(|err| format!("failed to write plan file: {err}"))?;
    }
    run_editor_on_file(path)?;
    let content =
        std::fs::read_to_string(path).map_err(|err| format!("failed to read plan file: {err}"))?;
    let modified = content != initial_content;
    Ok((content, modified))
}

pub(super) fn resolve_editor_command() -> Result<(String, Vec<String>), String> {
    let raw = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string());

    parse_editor_command(&raw)
}

pub(super) fn parse_editor_command(raw: &str) -> Result<(String, Vec<String>), String> {
    let mut parts =
        shlex::split(raw).ok_or_else(|| format!("failed to parse editor command `{raw}`"))?;
    if parts.is_empty() {
        return Err("editor command resolved to an empty argv".to_string());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

pub(super) fn run_editor_on_file(path: &std::path::Path) -> Result<(), String> {
    let (program, args) = resolve_editor_command()?;
    let status = std::process::Command::new(&program)
        .args(args)
        .arg(path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to launch editor `{program}`: {err}"))?;

    if !status.success() {
        return Err(format!("editor `{program}` exited with status {status}"));
    }

    Ok(())
}

/// Cap `text` at the smaller of `max_bytes` or `max_lines`, appending a
/// short notice when truncation occurs. Splits on char boundaries so
/// UTF-8 stays intact even when the byte limit lands mid-codepoint.
pub(super) fn truncate_output(text: String, max_bytes: usize, max_lines: usize) -> String {
    let line_count = text.lines().count();
    let byte_over = text.len() > max_bytes;
    if !byte_over && line_count <= max_lines {
        return text;
    }
    let mut truncated: String = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if truncated.len() > max_bytes {
        let cut = truncated
            .char_indices()
            .take_while(|(i, _)| *i <= max_bytes)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        truncated.truncate(cut);
    }
    truncated.push_str("\n… (truncated)");
    truncated
}
use anyhow::Result;
use coco_query::{CoreEvent, ServerNotification};
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;
use tracing::warn;

use super::{PendingEditorRequest, interactive_session, interactive_target};
