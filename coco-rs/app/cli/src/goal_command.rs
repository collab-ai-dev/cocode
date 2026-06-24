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
        text.push_str(&format!(
            "\nLast check: {}",
            format_goal_last_reason(reason)
        ));
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

pub fn active_goal(condition: String, tokens_at_start: i64) -> coco_types::ActiveGoal {
    coco_types::ActiveGoal {
        condition,
        iterations: 0,
        set_at_ms: unix_time_ms(),
        tokens_at_start,
        last_reason: None,
    }
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

pub fn remove_all_goal_hooks(
    runtime: &Arc<crate::session_runtime::SessionRuntime>,
) -> Vec<coco_hooks::HookDefinition> {
    runtime
        .hook_registry()
        .remove_matching_hooks(is_managed_goal_hook)
}

pub fn prompt_from_hook(hook: &coco_hooks::HookDefinition) -> Option<String> {
    match &hook.handler {
        coco_hooks::HookHandler::Prompt { prompt, .. } => Some(prompt.clone()),
        _ => None,
    }
}

pub fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_goal_status_matches_noninteractive_contract() {
        let mut goal = coco_types::ActiveGoal {
            condition: "finish migration".to_string(),
            iterations: 0,
            set_at_ms: 0,
            tokens_at_start: 0,
            last_reason: None,
        };
        assert_eq!(
            format_active_goal_status(&goal),
            "Goal active: finish migration (not yet evaluated)"
        );

        goal.iterations = 2;
        goal.last_reason = Some(" tests still failing\n rerun needed ".to_string());
        assert_eq!(
            format_active_goal_status(&goal),
            "Goal active: finish migration (2 turns)\nLast check: tests still failing rerun needed"
        );
    }

    #[test]
    fn find_last_achieved_goal_skips_clear_sentinel() {
        let clear = coco_messages::Message::Attachment(
            coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
                met: true,
                condition: "cleared".to_string(),
                sentinel: true,
                ..Default::default()
            }),
        );
        let achieved = coco_messages::Message::Attachment(
            coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
                met: true,
                condition: "done".to_string(),
                iterations: Some(3),
                sentinel: false,
                ..Default::default()
            }),
        );
        let history = vec![Arc::new(achieved), Arc::new(clear)];

        let found = find_last_achieved_goal(&history).expect("achieved goal");

        assert_eq!(found.condition, "done");
        assert_eq!(found.iterations, Some(3));
    }

    #[test]
    fn goal_hook_matcher_requires_managed_session_stop_prompt() {
        let mut hook = managed_goal_hook("done".to_string());

        assert!(is_managed_goal_hook(&hook));
        hook.managed_by = None;
        assert!(!is_managed_goal_hook(&hook));
        hook.managed_by = Some(coco_hooks::ManagedHookKind::Goal);
        hook.scope = coco_types::HookScope::User;
        assert!(!is_managed_goal_hook(&hook));
    }
}
