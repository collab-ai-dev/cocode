use super::*;
pub(super) fn parse_headless_goal_slash(text: &str) -> Option<&str> {
    let body = text.trim().strip_prefix('/')?;
    if body == "goal" {
        return Some("");
    }
    let args = body.strip_prefix("goal")?;
    if !args.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(args.trim_start())
}

pub(super) fn append_headless_slash_text(
    messages: &mut Vec<std::sync::Arc<coco_messages::Message>>,
    command: &str,
    args: &str,
    text: &str,
) {
    messages.extend(
        coco_messages::build_slash_command_messages(command, args, text, false)
            .into_iter()
            .map(std::sync::Arc::new),
    );
}

pub(super) fn append_headless_goal_status(
    messages: &mut Vec<std::sync::Arc<coco_messages::Message>>,
    payload: coco_types::GoalStatusPayload,
) {
    messages.push(std::sync::Arc::new(coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(payload),
    )));
}

pub(super) async fn headless_local_goal_text_outcome(
    cli: &AgentHostOptions,
    cwd: &Path,
    session_id: &coco_types::SessionId,
    args: &str,
    response_text: String,
    prior_messages: Vec<std::sync::Arc<coco_messages::Message>>,
) -> RunChatOutcome {
    let mut local_messages = Vec::new();
    append_headless_slash_text(&mut local_messages, "goal", args, &response_text);
    persist_headless_local_transcript_messages(
        cli,
        cwd,
        session_id,
        &prior_messages,
        &local_messages,
    )
    .await;
    let mut final_messages = prior_messages;
    final_messages.extend(local_messages);
    headless_text_outcome(
        cli,
        cwd,
        response_text,
        final_messages,
        "local".to_string(),
        None,
        coco_types::PermissionMode::default(),
        false,
        None,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn headless_text_outcome(
    cli: &AgentHostOptions,
    cwd: &Path,
    response_text: String,
    final_messages: Vec<std::sync::Arc<coco_messages::Message>>,
    model_id: String,
    provider_api: Option<coco_types::ProviderApi>,
    permission_mode: coco_types::PermissionMode,
    bypass_permissions_available: bool,
    permission_notification: Option<String>,
    installed_fallback_count: usize,
) -> RunChatOutcome {
    RunChatOutcome {
        response_text,
        turns: 0,
        total_usage: TokenUsage::default(),
        cost_tracker: CostTracker::new(),
        model_id,
        provider_api,
        permission_mode,
        bypass_permissions_available,
        permission_notification,
        duration_ms: 0,
        duration_api_ms: 0,
        budget_exhausted: false,
        cancelled: false,
        last_continue_reason: None,
        installed_fallback_count,
        final_messages,
        effective_cwd: cwd.to_path_buf(),
        additional_dirs: resolve_additional_dirs(cli, cwd),
        tool_filter_summary: summarize_tool_filter(cli),
        app_server_shutdown: ShutdownDrainOutcome::Clean,
        event_hub_shutdown: ShutdownDrainOutcome::Clean,
    }
}

pub(super) async fn persist_headless_local_transcript_messages(
    cli: &AgentHostOptions,
    cwd: &Path,
    session_id: &coco_types::SessionId,
    prior_messages: &[std::sync::Arc<coco_messages::Message>],
    local_messages: &[std::sync::Arc<coco_messages::Message>],
) {
    if cli.no_session_persistence || local_messages.is_empty() {
        return;
    }
    let paths = crate::paths::project_paths(cwd);
    let store = coco_session::TranscriptStore::new(paths);
    let mut seen: std::collections::HashSet<uuid::Uuid> = prior_messages
        .iter()
        .filter_map(|message| message.uuid().copied())
        .collect();
    let starting_parent_uuid = prior_messages
        .iter()
        .rev()
        .find_map(|message| message.uuid().map(std::string::ToString::to_string));
    let git_branch = coco_git::get_current_branch(cwd)
        .ok()
        .flatten()
        .filter(|branch| !branch.is_empty());
    let options = coco_session::storage::ChainWriteOptions {
        cwd: cwd.display().to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        is_sidechain: false,
        agent_id: None,
        starting_parent_uuid,
        git_branch,
    };
    let message_refs: Vec<&coco_messages::Message> =
        local_messages.iter().map(AsRef::as_ref).collect();
    if let Err(e) =
        store.append_message_chain(session_id.as_str(), message_refs, &mut seen, options)
    {
        tracing::warn!(error = %e, session_id = %session_id, "failed to persist headless local transcript messages");
    }
}

/// Translate `--allowed-tools` / `--disallowed-tools` into a
/// [`coco_types::ToolFilter`]. Empty inputs ⇒ `unrestricted()`.
pub(super) fn build_tool_filter(cli: &AgentHostOptions) -> coco_types::ToolFilter {
    if cli.allowed_tools.is_empty() && cli.disallowed_tools.is_empty() {
        return coco_types::ToolFilter::unrestricted();
    }
    coco_types::ToolFilter::new(cli.allowed_tools.clone(), cli.disallowed_tools.clone())
}

/// Lightweight summary of the resolved tool filter for [`RunChatOutcome`].
/// Returns `None` when both `--allowed-tools` and `--disallowed-tools`
/// are empty (caller can equate that with `unrestricted`).
pub(super) fn summarize_tool_filter(cli: &AgentHostOptions) -> Option<ToolFilterSummary> {
    if cli.allowed_tools.is_empty() && cli.disallowed_tools.is_empty() {
        return None;
    }
    let mut allowed = cli.allowed_tools.clone();
    let mut disallowed = cli.disallowed_tools.clone();
    allowed.sort();
    disallowed.sort();
    Some(ToolFilterSummary {
        allowed,
        disallowed,
    })
}

/// Resolve `--add-dir` flag values to absolute paths anchored at `cwd`.
/// Callers that need the rendered display form for the env block should use
/// [`resolve_additional_dirs_display`].
pub(crate) fn resolve_additional_dirs(cli: &AgentHostOptions, cwd: &Path) -> Vec<PathBuf> {
    cli.add_dir
        .iter()
        .map(|raw| {
            let p = Path::new(raw);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            }
        })
        .collect()
}

/// Public sibling of [`resolve_additional_dirs`] returning the display
/// form (`String`) that flows into `coco_context::build_system_prompt`'s
/// `additional_working_directories` slot. Single source of truth for the
/// `--add-dir` → env-block transformation; previously duplicated in
/// `session_bootstrap.rs` and `headless::compose_system_prompt`.
pub fn resolve_additional_dirs_display(cli: &AgentHostOptions, cwd: &Path) -> Vec<String> {
    resolve_additional_dirs(cli, cwd)
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}
