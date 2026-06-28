use std::sync::Arc;

pub const HOOKS_GATE_MESSAGE: &str = "/goal can't run while hooks are restricted (disableAllHooks or allowManagedHooksOnly is set in settings or by policy).";
pub const TRUST_GATE_MESSAGE: &str = "/goal is only available in trusted workspaces. Restart, accept the trust dialog, and try again.";

pub fn build_goal_kickoff_prompt(condition: &str) -> String {
    format!(
        "A session-scoped Stop hook is now active with condition: \"{condition}\". Briefly acknowledge the goal, then immediately start (or continue) working toward it — treat the condition itself as your directive and do not pause to ask the user what to do. The hook will block stopping until the condition holds. It auto-clears once the condition is met — do not tell the user to run `/goal clear` after success; that's only for clearing a goal early."
    )
}

pub fn format_achieved_goal_status(goal: &coco_types::GoalStatusPayload) -> String {
    let mut text = format!("Goal achieved: {}", goal.condition);
    let mut stats = Vec::new();
    if let Some(duration_ms) = goal.duration_ms {
        stats.push(format!("{} ms", duration_ms.max(0)));
    }
    if let Some(iterations) = goal.iterations {
        let suffix = if iterations == 1 { "" } else { "s" };
        stats.push(format!("{iterations} turn{suffix}"));
    }
    if let Some(tokens) = goal.tokens {
        stats.push(format!("{} tokens", tokens.max(0)));
    }
    if !stats.is_empty() {
        text.push_str(&format!("\nStats: {}", stats.join(" · ")));
    }
    text
}

pub fn format_active_goal_status(goal: &coco_types::ActiveGoal) -> String {
    let status = if goal.iterations == 0 {
        "not yet evaluated".to_string()
    } else {
        let suffix = if goal.iterations == 1 { "" } else { "s" };
        format!("{} turn{suffix}", goal.iterations)
    };
    let mut text = format!("Goal active: {} ({status})", goal.condition);
    if let Some(reason) = goal
        .last_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        text.push('\n');
        text.push_str(&format_goal_last_reason(reason));
    }
    text
}

pub fn format_goal_last_reason(reason: &str) -> String {
    reason
        .trim()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn find_last_achieved_goal(
    messages: &[Arc<coco_messages::Message>],
) -> Option<coco_types::GoalStatusPayload> {
    messages.iter().rev().find_map(|message| {
        let coco_messages::Message::Attachment(attachment) = message.as_ref() else {
            return None;
        };
        let coco_messages::AttachmentBody::Silent(coco_messages::SilentPayload::GoalStatus(
            payload,
        )) = &attachment.body
        else {
            return None;
        };
        if payload.met && !payload.sentinel {
            Some(payload.clone())
        } else {
            None
        }
    })
}

pub fn find_latest_goal_status(
    messages: &[Arc<coco_messages::Message>],
) -> Option<coco_types::GoalStatusPayload> {
    messages.iter().rev().find_map(|message| {
        let coco_messages::Message::Attachment(attachment) = message.as_ref() else {
            return None;
        };
        let coco_messages::AttachmentBody::Silent(coco_messages::SilentPayload::GoalStatus(
            payload,
        )) = &attachment.body
        else {
            return None;
        };
        Some(payload.clone())
    })
}

pub fn format_latest_goal_history_status(
    messages: &[Arc<coco_messages::Message>],
) -> Option<String> {
    let latest = find_latest_goal_status(messages)?;
    if latest.met || latest.failed {
        return (latest.met && !latest.failed && !latest.sentinel)
            .then(|| format_achieved_goal_status(&latest));
    }
    let goal = coco_types::ActiveGoal {
        condition: latest.condition,
        iterations: latest.iterations.unwrap_or_default(),
        set_at_ms: 0,
        tokens_at_start: 0,
        last_reason: latest.reason,
    };
    Some(format_active_goal_status(&goal))
}

pub fn find_restorable_goal_condition(messages: &[Arc<coco_messages::Message>]) -> Option<String> {
    for message in messages.iter().rev() {
        let coco_messages::Message::Attachment(attachment) = message.as_ref() else {
            continue;
        };
        let coco_messages::AttachmentBody::Silent(coco_messages::SilentPayload::GoalStatus(
            payload,
        )) = &attachment.body
        else {
            continue;
        };
        return if payload.met || payload.failed {
            None
        } else {
            Some(payload.condition.clone())
        };
    }
    None
}

pub fn active_goal(condition: String, tokens_at_start: i64) -> coco_types::ActiveGoal {
    coco_types::ActiveGoal {
        condition,
        iterations: 0,
        set_at_ms: unix_time_ms(),
        tokens_at_start,
        last_reason: None,
    }
}

pub async fn restore_goal_from_history(
    messages: &[Arc<coco_messages::Message>],
    app_state: &tokio::sync::RwLock<coco_types::ToolAppState>,
    hook_registry: &coco_hooks::HookRegistry,
    tokens_at_start: i64,
    gate: GoalGate,
) -> Option<coco_types::ActiveGoal> {
    if gate.hooks_restricted || gate.trust_rejected {
        hook_registry.remove_matching_hooks(is_managed_goal_hook);
        app_state.write().await.active_goal = None;
        return None;
    }

    let Some(condition) = find_restorable_goal_condition(messages) else {
        hook_registry.remove_matching_hooks(is_managed_goal_hook);
        app_state.write().await.active_goal = None;
        return None;
    };
    hook_registry.remove_matching_hooks(is_managed_goal_hook);
    let goal = active_goal(condition.clone(), tokens_at_start);
    app_state.write().await.active_goal = Some(goal.clone());
    hook_registry.register(managed_goal_hook(condition));
    Some(goal)
}

pub fn active_goal_changed_notification(
    goal: Option<coco_types::ActiveGoal>,
) -> coco_types::ServerNotification {
    coco_types::ServerNotification::ActiveGoalChanged(Box::new(
        coco_types::ActiveGoalChangedParams { goal },
    ))
}

pub fn goal_status_sentinel(met: bool, condition: String) -> coco_types::GoalStatusPayload {
    coco_types::GoalStatusPayload {
        met,
        condition,
        sentinel: true,
        ..Default::default()
    }
}

pub fn managed_goal_hook(condition: String) -> coco_hooks::HookDefinition {
    coco_hooks::HookDefinition {
        event: coco_types::HookEventType::Stop,
        matcher: None,
        handler: coco_hooks::HookHandler::Prompt {
            prompt: condition,
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: coco_types::HookScope::Session,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
        managed_by: Some(coco_hooks::ManagedHookKind::Goal),
    }
}

pub fn is_managed_goal_hook(hook: &coco_hooks::HookDefinition) -> bool {
    hook.managed_by == Some(coco_hooks::ManagedHookKind::Goal)
        && hook.event == coco_types::HookEventType::Stop
        && hook.scope == coco_types::HookScope::Session
}

pub fn prompt_from_hook(hook: &coco_hooks::HookDefinition) -> Option<String> {
    match &hook.handler {
        coco_hooks::HookHandler::Prompt { prompt, .. } => Some(prompt.clone()),
        _ => None,
    }
}

/// Precomputed gate state for a `/goal set`.
///
/// `hooks_restricted` mirrors `disable_all_hooks || allow_managed_hooks_only` —
/// `/goal` *is* a Stop hook, so when hooks are restricted the feature is
/// structurally unavailable. `trust_rejected` is the **interactive-only**
/// workspace-trust check; it is always `false` for non-interactive surfaces
/// (SDK / headless), which deliberately skip the trust gate (the upstream
/// carve-out for headless / CI usage).
#[derive(Debug, Clone, Copy, Default)]
pub struct GoalGate {
    pub hooks_restricted: bool,
    pub trust_rejected: bool,
}

/// Side effects a `/goal` dispatch resolves to, decoupled from each runner's
/// I/O substrate (TUI events vs SDK history vs headless `Vec`). The caller
/// performs the actual emit / append / engine-run via its own sinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalOutcome {
    /// Show `text`; no transcript mutation, no engine run. Covers status,
    /// "No goal set", and both gate rejections.
    Text(String),
    /// Append the `status` sentinel attachment, then show `text`. Emitted by
    /// `clear` when a goal was actually active.
    StatusThenText {
        status: coco_types::GoalStatusPayload,
        text: String,
    },
    /// Append the `status` sentinel, show `text`, then run the engine with
    /// `kickoff` as the user prompt. Emitted by a successful `set`.
    SetAndRun {
        status: coco_types::GoalStatusPayload,
        text: String,
        kickoff: String,
    },
}

/// The command-echo argument string for a `/goal` request, matching the
/// upstream transcript framing: empty for status, `clear` for any clear
/// keyword, the raw condition for a set.
pub fn goal_display_args(request: &coco_commands::GoalCommandRequest) -> &str {
    match request {
        coco_commands::GoalCommandRequest::Status => "",
        coco_commands::GoalCommandRequest::Clear => "clear",
        coco_commands::GoalCommandRequest::Set { condition } => condition,
    }
}

/// Single source of truth for `/goal` dispatch across the TUI, SDK, and
/// headless runners. Performs the app-state and hook-registry mutations and
/// returns the I/O the caller must carry out via its own sinks.
///
/// `history` is the transcript scanned for the latest goal marker when no
/// goal is active; `tokens_at_start` is the session output-token baseline
/// recorded on a fresh `set`. The hooks gate is checked before the trust gate
/// so a hooks-restricted session reports the structural reason rather than a
/// misleading trust message.
pub async fn resolve_goal_request(
    request: coco_commands::GoalCommandRequest,
    app_state: &tokio::sync::RwLock<coco_types::ToolAppState>,
    hook_registry: &coco_hooks::HookRegistry,
    history: &[Arc<coco_messages::Message>],
    tokens_at_start: i64,
    gate: GoalGate,
) -> GoalOutcome {
    match request {
        coco_commands::GoalCommandRequest::Status => {
            let active = app_state.read().await.active_goal.clone();
            let text = match active {
                Some(goal) => format_active_goal_status(&goal),
                None => format_latest_goal_history_status(history)
                    .unwrap_or_else(|| "No goal set. Usage: `/goal <condition>`".to_string()),
            };
            GoalOutcome::Text(text)
        }
        coco_commands::GoalCommandRequest::Clear => {
            let removed = hook_registry.remove_matching_hooks(is_managed_goal_hook);
            let active_condition = app_state
                .write()
                .await
                .active_goal
                .take()
                .map(|goal| goal.condition);
            match active_condition.or_else(|| removed.iter().find_map(prompt_from_hook)) {
                Some(condition) => GoalOutcome::StatusThenText {
                    status: goal_status_sentinel(true, condition.clone()),
                    text: format!("Goal cleared: {condition}"),
                },
                None => GoalOutcome::Text("No goal set".to_string()),
            }
        }
        coco_commands::GoalCommandRequest::Set { condition } => {
            if gate.hooks_restricted {
                return GoalOutcome::Text(HOOKS_GATE_MESSAGE.to_string());
            }
            if gate.trust_rejected {
                return GoalOutcome::Text(TRUST_GATE_MESSAGE.to_string());
            }
            hook_registry.remove_matching_hooks(is_managed_goal_hook);
            app_state.write().await.active_goal =
                Some(active_goal(condition.clone(), tokens_at_start));
            hook_registry.register(managed_goal_hook(condition.clone()));
            GoalOutcome::SetAndRun {
                status: goal_status_sentinel(false, condition.clone()),
                text: format!("Goal set: {condition}"),
                kickoff: build_goal_kickoff_prompt(&condition),
            }
        }
    }
}

pub fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "goal_command.test.rs"]
mod tests;
