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

/// Typed error from `prepare_agent_create`. Variants map 1:1 to
/// `coco_tui::state::WizardError`; the CLI bridge produces them so
/// the TUI side can stamp the wizard's `error` slot with a typed
/// payload instead of trying to parse a stringly error.
#[derive(Debug)]
pub(super) enum CreateAgentError {
    NonWritableSource(coco_types::AgentSource),
    AlreadyExists(std::path::PathBuf),
    Io(String),
}

impl CreateAgentError {
    pub(super) fn to_user_string(&self) -> String {
        match self {
            Self::NonWritableSource(s) => {
                format!("source {s:?} is not writable from the wizard")
            }
            Self::AlreadyExists(p) => {
                format!("agent file already exists at {}", p.display())
            }
            Self::Io(m) => m.clone(),
        }
    }
}

/// Stage the new-agent markdown file ahead of the `$EDITOR` fork.
/// 1. Resolves the target directory via
/// [`coco_subagent::resolve_writable_agent_dir`].
/// 2. Pulls the live catalog snapshot **once** so the colour picker
/// and the post-write reload share the same view.
/// 3. Wraps `create_dir_all` + `write` in `spawn_blocking` so a slow
/// disk doesn't stall the async runtime.
/// 4. Refuses to overwrite an existing file.
/// The caller then hands off to the standard editor flow.
pub(super) async fn prepare_agent_create(
    session: &crate::session_runtime::SessionHandle,
    name: &str,
    description: &str,
    source: coco_types::AgentSource,
) -> Result<std::path::PathBuf, CreateAgentError> {
    let runtime = session;
    // Snapshot the catalog ONCE — the colour picker reads it, and
    // the post-write reload supersedes it on its own. Repeated
    // `agent_catalog_snapshot().await` calls add lock churn for no
    // benefit since the data is immutable per snapshot.
    let snapshot = runtime.agent_catalog_snapshot().await;
    let color = coco_subagent::next_unused_color(&snapshot);

    let name_owned = name.to_string();
    let description_owned = description.to_string();
    let cwd = runtime.current_engine_config().await.workspace_cwd();
    let blocking =
        tokio::task::spawn_blocking(move || -> Result<std::path::PathBuf, CreateAgentError> {
            let config_home = coco_config::global_config::config_home();
            let dir = coco_subagent::resolve_writable_agent_dir(source, &config_home, &cwd)
                .ok_or(CreateAgentError::NonWritableSource(source))?;
            std::fs::create_dir_all(&dir).map_err(|err| CreateAgentError::Io(err.to_string()))?;
            let path = dir.join(format!("{name_owned}.md"));
            if path.exists() {
                return Err(CreateAgentError::AlreadyExists(path));
            }
            let template = build_agent_template(&name_owned, &description_owned, color);
            std::fs::write(&path, template).map_err(|err| CreateAgentError::Io(err.to_string()))?;
            Ok(path)
        })
        .await
        .map_err(|join_err| CreateAgentError::Io(format!("write task panicked: {join_err}")))??;

    // Pre-warm the catalog so observers see the new file without
    // waiting on the editor to exit — handy for SDK consumers that
    // listen to `agents/refreshed` between the create and the edit.
    runtime.reload_agent_catalog().await;
    Ok(blocking)
}

/// Build the markdown body written by the create wizard. Frontmatter carries the wizard inputs plus
/// an auto-assigned color from the eight-color palette so new agents
/// land with visual distinction in the Library list.
pub(super) fn build_agent_template(
    name: &str,
    description: &str,
    color: Option<coco_types::AgentColorName>,
) -> String {
    let description_yaml = yaml_single_quote(description);
    let color_line = match color {
        Some(c) => format!("color: {}\n", c.as_str()),
        None => String::new(),
    };
    format!(
        "---\n\
         name: {name}\n\
         description: {description_yaml}\n\
         {color_line}\
         ---\n\
         \n\
         # {name}\n\
         \n\
         <!-- Describe how this agent should behave. Frontmatter \
         fields you can add: tools, model, memory, isolation, \
         background, maxTurns, initialPrompt. -->\n",
    )
}

/// Encode a single-line string as a YAML single-quoted scalar. YAML
/// single-quoted form is the simplest robust escape: the only
/// in-string syntax is the single quote itself, which doubles to
/// `''`. Control characters and backslashes pass through literally,
/// dodging the double-quote escape surface entirely.
/// The wizard's `wizard_input_char` already rejects literal newlines
/// (`InsertNewline` is unbound) and control characters on the
/// description step, so by the time text reaches here it's a single
/// physical line — exactly what the YAML single-quoted format
/// requires.
pub(super) fn yaml_single_quote(s: &str) -> String {
    let escaped = s.replace('\'', "''");
    format!("'{escaped}'")
}

/// Fork `$EDITOR` against the agent markdown file. On clean exit
/// the runner triggers a `reload_agent_catalog()` **only when the
/// file actually changed** so an editor session that quit without
/// saving doesn't churn the catalog. Falls back to reload on any
/// mtime-read error so a missing-stat doesn't strand the dialog.
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
            session.reload_agent_catalog().await;
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

/// Build a fresh `AgentsDialogPayload` from the live catalog snapshot
/// and push it to the TUI via `OpenAgentsDialog`. Used after CRUD
/// (`OpenAgentEditor` exit, `DeleteAgentFile`) so the dialog refreshes
/// in place rather than waiting for the user to re-issue `/agents`.
pub(super) async fn refresh_agents_dialog(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    let snapshot = runtime.agent_catalog_snapshot().await;

    let active_source: std::collections::BTreeMap<String, coco_types::AgentSource> = snapshot
        .active()
        .map(|d| (d.name.clone(), d.source))
        .collect();

    let entries: Vec<coco_types::AgentsDialogEntry> = snapshot
        .all()
        .iter()
        .map(|loaded| {
            let def = &loaded.definition;
            let is_overridden = active_source
                .get(&def.name)
                .map(|winning| *winning != def.source)
                .unwrap_or(false);
            coco_types::AgentsDialogEntry {
                name: def.name.clone(),
                description: def.description.clone().unwrap_or_default(),
                source: def.source,
                color: def.color,
                is_overridden,
                source_path: loaded.path.clone(),
            }
        })
        .collect();
    let payload = coco_types::AgentsDialogPayload { entries };
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenAgentsDialog { payload }))
        .await;
}

/// Build a `PermissionsEditorPayload` snapshot from the on-disk settings
/// stores for the `/permissions` overlay. Reads every file-backed rule
/// (user / project / local / flag / policy) plus additional directories,
/// projecting them into the wire payload the TUI partitions into tabs.
pub(super) async fn build_permissions_editor_payload(
    session: &crate::session_runtime::SessionHandle,
) -> coco_types::PermissionsEditorPayload {
    let runtime = session;
    use coco_permissions::permissions_store::PermissionStore;

    let cwd = runtime.current_engine_config().await.workspace_cwd();
    let store = coco_permissions::SettingsPermissionStore::new(cwd.clone());

    // Reading several small JSON files — push onto the blocking pool so a
    // slow filesystem can't stall the runner's command loop.
    let (rules, directories, managed_only) = tokio::task::spawn_blocking(move || {
        let by_behavior = store.load_all_rules();
        let rules: Vec<coco_types::PermissionsEditorRule> = by_behavior
            .allow
            .into_iter()
            .chain(by_behavior.ask)
            .chain(by_behavior.deny)
            .map(|r| coco_types::PermissionsEditorRule {
                behavior: r.behavior,
                source: r.source,
                tool_pattern: r.value.tool_pattern,
                rule_content: r.value.rule_content,
            })
            .collect();
        let directories: Vec<coco_types::PermissionsEditorDir> = store
            .load_additional_directories()
            .into_iter()
            .map(|(source, path)| coco_types::PermissionsEditorDir { path, source })
            .collect();
        // `show_always_allow_options()` is the inverse of managed-only.
        let managed_only = !store.show_always_allow_options();
        (rules, directories, managed_only)
    })
    .await
    .unwrap_or_else(|_| (Vec::new(), Vec::new(), false));

    coco_types::PermissionsEditorPayload {
        rules,
        directories,
        cwd: cwd.to_string_lossy().into_owned(),
        managed_only,
    }
}

/// Re-emit `OpenPermissionsEditor` with a fresh snapshot so the open
/// overlay refreshes in place after a persisted edit.
pub(super) async fn refresh_permissions_editor(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let payload = build_permissions_editor_payload(session).await;
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
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .apply_permission_update(
            local_app_server_bridge.handler(),
            coco_types::ApplyPermissionUpdateParams {
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
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .reset_session_permission_rules(local_app_server_bridge.handler())
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
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> bool {
    if let Err(error) = local_app_server_bridge
        .client()
        .set_agent_color(
            local_app_server_bridge.handler(),
            coco_types::SetAgentColorParams { color },
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
use coco_query::CoreEvent;
use coco_query::ServerNotification;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;
use tracing::warn;

use super::PendingEditorRequest;
