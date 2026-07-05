//! Shared helpers for the shell tools' (`Bash` / `PowerShell`) background
//! path. A backgrounded command's result carries `backgroundTaskId` +
//! `outputPath`, and the model-facing notice names both. BashTool
//! `backgroundInfo`
//! Background task output persistence.
//! the deterministic `{session_dir}/{task_id}.output` location here.

use serde_json::Value;

use super::bash_advanced::ASSISTANT_BLOCKING_BUDGET_MS;

/// Which mechanism moved a command to the background. Drives the
/// model-facing notice. Mirrors the three TS flags
/// (`assistantAutoBackgrounded` / `backgroundedByUser` / neither).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackgroundKind {
    /// Foreground→background auto-promotion after the command exceeded the
    /// assistant blocking budget.
    AssistantAuto,
    /// User-initiated (Ctrl+B). The TUI keystroke is not yet wired.
    User,
    /// Explicit `run_in_background: true`.
    Explicit,
}

impl BackgroundKind {
    /// Read the discriminant off a background result envelope.
    pub(crate) fn from_result(data: &Value) -> Self {
        let flag = |key: &str| data.get(key).and_then(Value::as_bool).unwrap_or(false);
        if flag("assistantAutoBackgrounded") {
            Self::AssistantAuto
        } else if flag("backgroundedByUser") {
            Self::User
        } else {
            Self::Explicit
        }
    }
}

/// Model-facing notice for a command that moved to the background.
pub(crate) fn format_background_notice(
    kind: BackgroundKind,
    task_id: &str,
    output_path: &str,
) -> String {
    match kind {
        BackgroundKind::AssistantAuto => {
            let budget_seconds = ASSISTANT_BLOCKING_BUDGET_MS / 1000;
            format!(
                "Command exceeded the assistant-mode blocking budget ({budget_seconds}s) and was moved to the background with ID: {task_id}. It is still running — you will be notified when it completes. Future long-running shell commands should use run_in_background: true to keep this conversation responsive. Output is being written to: {output_path}."
            )
        }
        BackgroundKind::User => format!(
            "Command was manually backgrounded by user with ID: {task_id}. Output is being written to: {output_path}."
        ),
        BackgroundKind::Explicit => format!(
            "Command running in background with ID: {task_id}. Output is being written to: {output_path}."
        ),
    }
}

/// Resolve a background task's on-disk output file path to a display string,
/// empty when no task runtime is wired (tests / headless without a turn loop).
/// This is the path the model `Read`s — the deterministic
/// `{session_dir}/{task_id}.output` location owned by the task runtime.
pub(crate) async fn background_output_path(
    task_handle: &coco_tool_runtime::TaskHandleRef,
    task_id: &str,
) -> String {
    task_handle
        .output_file_path(task_id)
        .await
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "background_task.test.rs"]
mod tests;
