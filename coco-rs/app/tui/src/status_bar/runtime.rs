use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::presentation::context_usage::render_context_usage;
use crate::state::AppState;

const STATUS_LINE_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_LINE_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusLineUpdate {
    pub(crate) generation: u64,
    pub(crate) output: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusLineRuntime {
    generation: u64,
    generation_shared: Arc<AtomicU64>,
    last_request_key: Option<String>,
    last_success_output: Option<String>,
}

impl Default for StatusLineRuntime {
    fn default() -> Self {
        Self {
            generation: 0,
            generation_shared: Arc::new(AtomicU64::new(0)),
            last_request_key: None,
            last_success_output: None,
        }
    }
}

impl StatusLineRuntime {
    pub(crate) fn last_success(&self) -> Option<&str> {
        self.last_success_output
            .as_deref()
            .and_then(|output| output.lines().next())
    }

    pub(crate) fn request_refresh(
        &mut self,
        state: &AppState,
        tx: &mpsc::Sender<StatusLineUpdate>,
    ) {
        let Some(status_line) = state.ui.display_settings.status_line.as_ref() else {
            if self.last_request_key.take().is_some() {
                self.invalidate_pending();
            }
            return;
        };
        let command = status_line.as_command().command.trim().to_string();
        if command.is_empty() {
            if self.last_request_key.take().is_some() {
                self.invalidate_pending();
            }
            return;
        }

        let input = status_line_input(state);
        let input_json = match serde_json::to_string(&input) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "statusLine input serialization failed");
                return;
            }
        };
        let request_key = format!("{command}\n{input_json}");
        if self.last_request_key.as_deref() == Some(request_key.as_str()) {
            return;
        }
        self.last_request_key = Some(request_key);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.generation_shared.store(generation, Ordering::SeqCst);
        let generation_shared = self.generation_shared.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(STATUS_LINE_DEBOUNCE).await;
            if generation_shared.load(Ordering::SeqCst) != generation {
                return;
            }
            let output = run_status_line_command(&command, &input_json).await.ok();
            let _ = tx.send(StatusLineUpdate { generation, output }).await;
        });
    }

    pub(crate) fn apply_update(&mut self, update: StatusLineUpdate) -> bool {
        if update.generation != self.generation {
            return false;
        }
        let Some(output) = update.output else {
            return false;
        };
        if output.is_empty() {
            return false;
        }
        if self.last_success_output.as_deref() == Some(output.as_str()) {
            return false;
        }
        self.last_success_output = Some(output);
        true
    }

    fn invalidate_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.generation_shared
            .store(self.generation, Ordering::SeqCst);
    }
}

#[derive(Debug, Serialize)]
struct StatusLineInput {
    session_id: Option<String>,
    model: StatusLineModel,
    workspace: StatusLineWorkspace,
    version: &'static str,
    output_style: StatusLineOutputStyle,
    cost: serde_json::Value,
    context_window: serde_json::Value,
    exceeds_200k_tokens: bool,
    permission_mode: String,
    lsp: StatusLineLsp,
    mcp: StatusLineMcp,
}

#[derive(Debug, Serialize)]
struct StatusLineModel {
    id: String,
    display_name: String,
    provider: String,
}

#[derive(Debug, Serialize)]
struct StatusLineWorkspace {
    current_dir: Option<String>,
    project_dir: Option<String>,
    added_dirs: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StatusLineOutputStyle {
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct StatusLineLsp {
    active: bool,
}

#[derive(Debug, Serialize)]
struct StatusLineMcp {
    connected_servers: Vec<String>,
}

fn status_line_input(state: &AppState) -> StatusLineInput {
    let (provider, model_id) = state
        .session
        .model_by_role
        .get(&coco_types::ModelRole::Main)
        .map(|binding| (binding.provider.clone(), binding.model_id.clone()))
        .unwrap_or_else(|| (state.session.provider.clone(), state.session.model.clone()));
    let display_name = state
        .session
        .model_catalog
        .iter()
        .find(|entry| entry.provider == provider && entry.model_id == model_id)
        .map(|entry| entry.display_name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| model_id.clone());
    let current_dir = state.session.working_dir.clone();
    let project_dir = current_dir
        .as_deref()
        .and_then(|cwd| coco_git::find_canonical_git_root(std::path::Path::new(cwd)))
        .map(|path| path.display().to_string())
        .or_else(|| current_dir.clone());
    let context = render_context_usage(state);
    let context_window = match context {
        Some(usage) => json!({
            "used": usage.used,
            "total": usage.total,
            "percent": usage.percent,
        }),
        None => json!({
            "used": null,
            "total": null,
            "percent": null,
        }),
    };
    let exceeds_200k_tokens = context.is_some_and(|usage| usage.used > 200_000);
    let cost = match state.session.session_usage.as_ref() {
        Some(snapshot) => json!({
            "total_cost_usd": snapshot.totals.total_cost_usd,
            "input_cost_usd": snapshot.totals.input_cost_usd,
            "output_cost_usd": snapshot.totals.output_cost_usd,
            "cache_read_cost_usd": snapshot.totals.cache_read_cost_usd,
            "cache_creation_cost_usd": snapshot.totals.cache_creation_cost_usd,
            "request_count": snapshot.totals.request_count,
            "unpriced_request_count": snapshot.totals.unpriced_request_count,
        }),
        None => json!({
            "total_cost_usd": null,
            "input_cost_usd": null,
            "output_cost_usd": null,
            "cache_read_cost_usd": null,
            "cache_creation_cost_usd": null,
            "request_count": 0,
            "unpriced_request_count": 0,
        }),
    };

    StatusLineInput {
        session_id: state.session.session_id.clone(),
        model: StatusLineModel {
            id: model_id,
            display_name,
            provider,
        },
        workspace: StatusLineWorkspace {
            current_dir,
            project_dir,
            added_dirs: Vec::new(),
        },
        version: env!("CARGO_PKG_VERSION"),
        output_style: StatusLineOutputStyle {
            name: state.session.output_style.clone(),
        },
        cost,
        context_window,
        exceeds_200k_tokens,
        permission_mode: permission_mode_name(state.session.permission_mode).to_string(),
        lsp: StatusLineLsp {
            active: state.session.lsp_active,
        },
        mcp: StatusLineMcp {
            connected_servers: state
                .session
                .mcp_servers
                .iter()
                .filter(|server| server.connected)
                .map(|server| server.name.clone())
                .collect(),
        },
    }
}

fn permission_mode_name(mode: coco_types::PermissionMode) -> &'static str {
    match mode {
        coco_types::PermissionMode::Default => "default",
        coco_types::PermissionMode::Plan => "plan",
        coco_types::PermissionMode::BypassPermissions => "bypass_permissions",
        coco_types::PermissionMode::DontAsk => "dont_ask",
        coco_types::PermissionMode::AcceptEdits => "accept_edits",
        coco_types::PermissionMode::Auto => "auto",
        coco_types::PermissionMode::Bubble => "bubble",
    }
}

async fn run_status_line_command(command: &str, input_json: &str) -> anyhow::Result<String> {
    run_status_line_command_with_timeout(command, input_json, STATUS_LINE_TIMEOUT).await
}

async fn run_status_line_command_with_timeout(
    command: &str,
    input_json: &str,
    timeout_duration: Duration,
) -> anyhow::Result<String> {
    let mut command = shell_command(command);
    command.kill_on_drop(true);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }

    let mut stdout = child.stdout.take();
    let stdout_task = tokio::spawn(async move {
        let mut output = Vec::new();
        if let Some(stdout) = stdout.as_mut() {
            stdout.read_to_end(&mut output).await?;
        }
        Ok::<Vec<u8>, std::io::Error>(output)
    });

    let status = match timeout(timeout_duration, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                let _ = coco_utils_pty::process_group::kill_process_group_by_pid(pid);
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            anyhow::bail!("statusLine command timed out after {timeout_duration:?}");
        }
    };
    let output = stdout_task.await??;
    if !status.success() {
        anyhow::bail!("statusLine command exited with {status}");
    }
    let stdout = String::from_utf8_lossy(&output);
    let clean = strip_ansi(&stdout);
    Ok(normalize_output(&clean))
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd
    }
}

fn normalize_output(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "runtime.test.rs"]
mod tests;
