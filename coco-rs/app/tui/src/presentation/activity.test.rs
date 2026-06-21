use super::*;
use coco_types::ExpandedView;

use crate::state::AppState;
use crate::state::session::SubagentInstance;
use crate::state::session::SubagentStatus;

fn surface(view: TurnActivityView) -> ActivitySurfaceView {
    match view {
        TurnActivityView::Surface(surface) => surface,
        TurnActivityView::None => panic!("expected activity surface"),
    }
}

fn subagent() -> SubagentInstance {
    SubagentInstance {
        kind: crate::state::session::SubagentKind::Subagent,
        agent_id: "agent-1".into(),
        agent_type: "explore".into(),
        description: "scan".into(),
        status: SubagentStatus::Running,
        color: None,
        team_name: None,
        started_at_ms: None,
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cost_usd: 0.0,
    }
}

#[test]
fn turn_activity_view_renders_agents_when_narrow() {
    let mut state = AppState::default();
    state.session.subagents.push(subagent());
    state.session.expanded_view = ExpandedView::Teammates;

    let view = surface(turn_activity_view(&state, 20));

    assert_eq!(view.title, ActivityTitle::Agents);
}

#[test]
fn turn_activity_view_prefers_expanded_plan_activity() {
    let mut state = AppState::default();
    state.session.expanded_view = ExpandedView::Tasks;
    state
        .session
        .todos_by_agent
        .insert("main".into(), Vec::new());
    state.session.subagents.push(subagent());

    let view = surface(turn_activity_view(&state, 160));

    assert_eq!(view.title, ActivityTitle::Tasks);
    assert_eq!(view.border, ActivityBorder::Plan);
}

#[test]
fn turn_activity_view_uses_agents_for_teammates_view() {
    let mut state = AppState::default();
    state.session.expanded_view = ExpandedView::Teammates;
    state.session.subagents.push(subagent());

    let view = surface(turn_activity_view(&state, 160));

    assert_eq!(view.title, ActivityTitle::Agents);
    assert_eq!(view.border, ActivityBorder::Agents);
}

#[test]
fn turn_activity_view_falls_back_to_agents_when_present() {
    let mut state = AppState::default();
    state.session.subagents.push(subagent());

    let view = surface(turn_activity_view(&state, 160));

    assert_eq!(view.title, ActivityTitle::Agents);
}

#[test]
fn turn_activity_view_renders_stream_status_without_agents() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = AppState::default();
    state.session.stream_stall = true;

    let view = surface(turn_activity_view(&state, 160));

    assert_eq!(view.title, ActivityTitle::Activity);
    assert!(
        view.lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.text.contains("Stream stall"))
    );
}

#[test]
fn exit_plan_mode_tool_is_hidden_from_activity() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = AppState::default();
    // ExitPlanMode's UI is the "Ready to code?" dialog, so `start_tool` keeps
    // it out of the tool ledger entirely — no activity strip, no busy spinner,
    // no foreground-task count.
    state.session.start_tool(
        "call-1".into(),
        "ExitPlanMode".into(),
        &serde_json::Value::Null,
    );

    assert!(state.session.tool_executions.is_empty());
    assert!(matches!(
        turn_activity_view(&state, 160),
        TurnActivityView::None
    ));
}

#[test]
fn ask_user_question_tool_is_hidden_from_activity() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = AppState::default();
    // AskUserQuestion's UI is the question dialog; like ExitPlanMode it stays
    // out of the tool ledger (TS + codex-rs both suppress it).
    state.session.start_tool(
        "call-1".into(),
        "AskUserQuestion".into(),
        &serde_json::Value::Null,
    );

    assert!(state.session.tool_executions.is_empty());
    assert!(matches!(
        turn_activity_view(&state, 160),
        TurnActivityView::None
    ));
}

#[test]
fn ordinary_running_tool_renders_inline_not_in_panel() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = AppState::default();
    state.session.start_tool(
        "call-1".into(),
        "Bash".into(),
        &serde_json::json!({ "command": "ls -la" }),
    );

    // The tool is still tracked (live elapsed feeds the inline transcript
    // header `● Bash(ls -la) (Ns)`), but it no longer opens a separate
    // "Activity / Tools:" panel — codex / claude-code parity. With no
    // subagents and no stall, the activity surface is empty.
    assert_eq!(state.session.tool_executions.len(), 1);
    assert!(matches!(
        turn_activity_view(&state, 160),
        TurnActivityView::None
    ));
}

#[test]
fn turn_activity_view_keeps_stale_teammates_view_empty_without_agents() {
    let mut state = AppState::default();
    state.session.expanded_view = ExpandedView::Teammates;
    state
        .session
        .todos_by_agent
        .insert("main".into(), Vec::new());

    assert!(matches!(
        turn_activity_view(&state, 160),
        TurnActivityView::None
    ));
}

#[test]
fn subagent_row_truncates_long_description_and_tail() {
    // The Agents panel paints without `Wrap`, so an unbounded description or
    // live-action tail would hard-clip at the screen edge mid-word. Both must
    // be ellipsised so the stats stay on-screen.
    let mut state = AppState::default();
    let mut agent = subagent();
    agent.description = "Map every crate in the app, core, and services layers thoroughly".into();
    agent.last_tool_name =
        Some("args: medium thoroughness: explore crate-level CLAUDE.md files under coco-rs".into());
    state.session.subagents.push(agent);

    let view = surface(turn_activity_view(&state, 160));
    let text: String = view
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .map(|span| span.text.as_ref())
        .collect();

    assert!(text.contains('…'), "expected an ellipsis: {text}");
    assert!(
        !text.contains("services layers thoroughly"),
        "description tail should be truncated: {text}"
    );
    assert!(
        !text.contains("CLAUDE.md files under coco-rs"),
        "live-action tail should be truncated: {text}"
    );
}

fn per_line_text(view: &ActivitySurfaceView) -> Vec<String> {
    view.lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.text.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn switcher_focus_adds_cursor_and_hint_no_main_row() {
    // While focused, the Agents panel doubles as the switcher: a `❯` cursor on
    // the selected AGENT (no `◯ main` row) plus a key-hint line.
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = AppState::default();
    let mut a1 = subagent();
    a1.agent_id = "a1".into();
    a1.agent_type = "Explore".into();
    let mut a2 = subagent();
    a2.agent_id = "a2".into();
    a2.agent_type = "Plan".into();
    state.session.subagents = vec![a1, a2];
    state.ui.focus = crate::state::FocusTarget::AgentSwitcher;
    state.ui.agent_switcher_selected = 0; // first agent (Explore)

    let view = surface(turn_activity_view(&state, 160));
    let rows = per_line_text(&view);
    let text = rows.join("\n");
    assert!(!text.contains("◯ main"), "main row should be gone: {text}");
    let explore_row = rows
        .iter()
        .find(|r| r.contains("Explore"))
        .expect("explore row");
    assert!(
        explore_row.starts_with("❯ "),
        "selected agent missing cursor: {explore_row}"
    );
    let plan_row = rows.iter().find(|r| r.contains("Plan")).expect("plan row");
    assert!(plan_row.starts_with("  "), "unselected agent: {plan_row}");
    assert!(text.contains("← back"), "missing key hint: {text}");
    assert!(text.contains("x stop"), "missing stop hint: {text}");

    // The selected row recolors its WHOLE line to accent (every span), not just
    // the cursor — so the unselected Plan row keeps non-accent tones.
    let explore_line = view
        .lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.text.contains("Explore")))
        .expect("explore line");
    assert!(
        explore_line
            .spans
            .iter()
            .all(|s| s.tone == ActivityTone::Accent),
        "selected row should be all-accent",
    );
    let plan_line = view
        .lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.text.contains("Plan")))
        .expect("plan line");
    assert!(
        !plan_line
            .spans
            .iter()
            .all(|s| s.tone == ActivityTone::Accent),
        "unselected row must not be all-accent",
    );
}

#[test]
fn no_switcher_cursor_when_unfocused() {
    // Passive panel (no focus) must look unchanged — no `◯ main`, no hint.
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = AppState::default();
    state.session.subagents = vec![subagent()];
    let view = surface(turn_activity_view(&state, 160));
    let text: String = per_line_text(&view).join("\n");
    assert!(!text.contains("◯ main"), "main row leaked: {text}");
    assert!(
        !text.contains("← back"),
        "hint leaked when unfocused: {text}"
    );
}

#[test]
fn viewed_agent_gets_marker() {
    let mut state = AppState::default();
    let mut a1 = subagent();
    a1.agent_id = "a1".into();
    a1.agent_type = "Explore".into();
    state.session.subagents = vec![a1];
    state.session.viewing_agent_id = Some("a1".into());
    let view = surface(turn_activity_view(&state, 160));
    let text: String = per_line_text(&view).join("\n");
    assert!(text.contains('◀'), "viewed marker missing: {text}");
}

#[test]
fn inline_activity_height_caps_narrow_rows() {
    // Eight lines exceed the narrow budget (6), so the height caps at
    // budget + 1 border row = 7.
    let view = TurnActivityView::Surface(ActivitySurfaceView {
        title: ActivityTitle::Activity,
        border: ActivityBorder::Activity,
        lines: (0..8)
            .map(|i| ActivityLine::text(format!("row {i}"), ActivityTone::Text))
            .collect(),
    });

    assert_eq!(inline_activity_height(&view, 20, 60), 7);
}

#[test]
fn inline_activity_height_respects_available_height() {
    let view = TurnActivityView::Surface(ActivitySurfaceView {
        title: ActivityTitle::Activity,
        border: ActivityBorder::Activity,
        lines: vec![
            ActivityLine::text("one", ActivityTone::Text),
            ActivityLine::text("two", ActivityTone::Text),
        ],
    });

    assert_eq!(inline_activity_height(&view, 2, 120), 2);
}
