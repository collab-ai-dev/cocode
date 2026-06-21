use super::build_view;
use crate::state::AppState;
use crate::state::FocusTarget;

fn running_subagent(agent_id: &str, agent_type: &str) -> crate::state::SubagentInstance {
    crate::state::SubagentInstance {
        kind: crate::state::SubagentKind::Subagent,
        agent_id: agent_id.to_string(),
        agent_type: agent_type.to_string(),
        description: format!("desc-{agent_id}"),
        status: crate::state::SubagentStatus::Running,
        color: Some(coco_types::AgentColorName::Blue),
        team_name: None,
        started_at_ms: Some(0),
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        cost_usd: 0.0,
    }
}

#[test]
fn empty_without_agents() {
    let state = AppState::default();
    let view = build_view(&state);
    assert!(view.is_empty());
    assert_eq!(view.row_count(), 0);
}

#[test]
fn rows_are_running_agents_in_order_no_main() {
    let mut state = AppState::default();
    state.session.subagents = vec![
        running_subagent("a1", "Explore"),
        running_subagent("a2", "Plan"),
    ];
    let view = build_view(&state);
    assert!(!view.is_empty());
    assert_eq!(view.row_count(), 2); // agents only, no `◯ main`
    assert_eq!(view.rows[0].agent_type, Some("Explore"));
    assert_eq!(view.rows[1].agent_type, Some("Plan"));
}

#[test]
fn agent_label_prefers_latest_activity_summary() {
    let mut state = AppState::default();
    let mut agent = running_subagent("a1", "Explore");
    agent.recent_activities = vec![coco_types::TaskActivity {
        tool_name: "Grep".to_string(),
        summary: Some("3 patterns".to_string()),
    }];
    state.session.subagents = vec![agent];
    let view = build_view(&state);
    assert_eq!(view.rows[0].label, "3 patterns");
}

#[test]
fn focus_and_selection_reflect_ui_state() {
    let mut state = AppState::default();
    state.session.subagents = vec![
        running_subagent("a1", "Explore"),
        running_subagent("a2", "Plan"),
    ];
    state.ui.focus = FocusTarget::AgentSwitcher;
    state.ui.agent_switcher_selected = 1;
    let view = build_view(&state);
    assert!(view.focused);
    assert_eq!(view.selected, 1);
}

#[test]
fn viewing_index_marks_the_open_agent() {
    let mut state = AppState::default();
    state.session.subagents = vec![
        running_subagent("a1", "Explore"),
        running_subagent("a2", "Plan"),
    ];
    state.session.viewing_agent_id = Some("a2".to_string());
    let view = build_view(&state);
    // a2 is the second agent → row index 1 (agents-only indexing).
    assert_eq!(view.viewing, Some(1));
}

#[test]
fn selection_is_clamped_to_row_count() {
    let mut state = AppState::default();
    state.session.subagents = vec![running_subagent("a1", "Explore")];
    state.ui.agent_switcher_selected = 99;
    let view = build_view(&state);
    assert_eq!(view.selected, 0); // 1 agent → max index 0
}
