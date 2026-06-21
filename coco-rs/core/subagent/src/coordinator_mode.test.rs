use super::*;

#[test]
fn session_mode_from_metadata_str_parses_known_values() {
    assert_eq!(
        SessionMode::from_metadata_str("coordinator"),
        Some(SessionMode::Coordinator)
    );
    assert_eq!(
        SessionMode::from_metadata_str("normal"),
        Some(SessionMode::Normal)
    );
    assert_eq!(SessionMode::from_metadata_str("bogus"), None);
    assert_eq!(SessionMode::from_metadata_str(""), None);
}

fn features_with_agent_teams(enabled: bool) -> Features {
    let mut f = Features::empty();
    if enabled {
        f.enable(Feature::AgentTeams);
    }
    f
}

#[test]
fn is_coordinator_mode_requires_both_gates() {
    // Env var unset by default in test env — env gate is false.
    let f = features_with_agent_teams(true);
    assert!(!is_coordinator_mode(&f));

    // Env-only check decoupled from feature gate.
    let _ = is_coordinator_mode_env(); // just confirm it compiles & doesn't panic
}

#[test]
fn worker_tool_pool_simple_returns_three_tools_sorted() {
    let pool = worker_tool_pool(true);
    assert_eq!(pool, vec!["Bash", "Edit", "Read"]);
}

#[test]
fn worker_tool_pool_full_excludes_internal_worker_tools() {
    let pool = worker_tool_pool(false);
    // Internal-only tools must not leak to workers.
    assert!(!pool.contains(&"TeamCreate"));
    assert!(!pool.contains(&"TeamDelete"));
    assert!(!pool.contains(&"SendMessage"));
    assert!(!pool.contains(&"SyntheticOutput"));
    // Expected staples are present.
    assert!(pool.contains(&"Bash"));
    assert!(pool.contains(&"Read"));
    assert!(pool.contains(&"Edit"));
}

#[test]
fn worker_tool_pool_full_is_sorted() {
    let pool = worker_tool_pool(false);
    let mut sorted = pool.clone();
    sorted.sort_unstable();
    assert_eq!(pool, sorted);
}

#[test]
fn coordinator_user_context_empty_when_gate_off() {
    let f = features_with_agent_teams(false);
    let ctx = coordinator_user_context(&f, &[], None, false, false);
    assert!(ctx.is_empty());
}

#[test]
fn session_mode_switch_action_no_stored_is_noop() {
    assert_eq!(
        session_mode_switch_action(None, true),
        SessionModeSwitch::NoOp
    );
    assert_eq!(
        session_mode_switch_action(None, false),
        SessionModeSwitch::NoOp
    );
}

#[test]
fn session_mode_switch_action_matching_modes_noop() {
    assert_eq!(
        session_mode_switch_action(Some(SessionMode::Coordinator), true),
        SessionModeSwitch::NoOp
    );
    assert_eq!(
        session_mode_switch_action(Some(SessionMode::Normal), false),
        SessionModeSwitch::NoOp
    );
}

#[test]
fn session_mode_switch_action_resume_into_coordinator() {
    assert_eq!(
        session_mode_switch_action(Some(SessionMode::Coordinator), false),
        SessionModeSwitch::EnterCoordinator
    );
}

#[test]
fn session_mode_switch_action_resume_out_of_coordinator() {
    assert_eq!(
        session_mode_switch_action(Some(SessionMode::Normal), true),
        SessionModeSwitch::ExitCoordinator
    );
}

#[test]
fn session_mode_switch_warnings_match_ts_strings() {
    assert_eq!(SessionModeSwitch::NoOp.warning(), None);
    assert_eq!(
        SessionModeSwitch::EnterCoordinator.warning(),
        Some("Entered coordinator mode to match resumed session.")
    );
    assert_eq!(
        SessionModeSwitch::ExitCoordinator.warning(),
        Some("Exited coordinator mode to match resumed session.")
    );
}

#[test]
fn render_task_notification_completed_minimal() {
    let n = TaskNotification {
        task_id: "agent-a1b",
        status: TaskNotificationStatus::Completed,
        summary: "Agent \"investigate auth bug\" completed",
        result: None,
        usage: None,
    };
    let xml = render_task_notification(&n);
    assert!(xml.starts_with("<task-notification>\n"));
    assert!(xml.ends_with("</task-notification>"));
    assert!(xml.contains("<task-id>agent-a1b</task-id>\n"));
    assert!(xml.contains("<status>completed</status>\n"));
    assert!(xml.contains("<summary>Agent \"investigate auth bug\" completed</summary>\n"));
    // Optional sections are omitted when None.
    assert!(!xml.contains("<result>"));
    assert!(!xml.contains("<usage>"));
}

#[test]
fn render_task_notification_with_result_and_usage() {
    let n = TaskNotification {
        task_id: "agent-x",
        status: TaskNotificationStatus::Failed,
        summary: "failed: build error",
        result: Some("rustc error E0599..."),
        usage: Some(TaskNotificationUsage {
            total_tokens: 1234,
            tool_uses: 7,
            duration_ms: 12_500,
        }),
    };
    let xml = render_task_notification(&n);
    assert!(xml.contains("<status>failed</status>"));
    assert!(xml.contains("<result>rustc error E0599...</result>"));
    assert!(xml.contains("<total_tokens>1234</total_tokens>"));
    assert!(xml.contains("<tool_uses>7</tool_uses>"));
    assert!(xml.contains("<duration_ms>12500</duration_ms>"));
}

#[test]
fn task_notification_status_strings_match_ts() {
    assert_eq!(TaskNotificationStatus::Completed.as_str(), "completed");
    assert_eq!(TaskNotificationStatus::Failed.as_str(), "failed");
    assert_eq!(TaskNotificationStatus::Killed.as_str(), "killed");
}

#[test]
fn coordinator_system_prompt_contains_role_section_and_tool_names() {
    let p = coordinator_system_prompt(false);
    assert!(p.contains("You are Claude Code"));
    assert!(p.contains("## 1. Your Role"));
    assert!(p.contains("## 2. Your Tools"));
    assert!(p.contains("Agent"));
    assert!(p.contains("SendMessage"));
    assert!(p.contains("TaskStop"));
    // Worker-capabilities sentence — full mode language.
    assert!(p.contains("standard tools"));
}

#[test]
fn coordinator_system_prompt_simple_mode_uses_simple_capability_line() {
    let p = coordinator_system_prompt(true);
    assert!(p.contains("Bash, Read, and Edit"));
}
