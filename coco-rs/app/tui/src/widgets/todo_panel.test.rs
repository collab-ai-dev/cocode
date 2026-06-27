use super::append_lines;
use crate::presentation::activity::ActivityLine;
use crate::state::AppState;
use coco_types::TaskListStatus;
use coco_types::TaskRecord;
use coco_types::TodoRecord;

fn task(id: &str, subject: &str, status: TaskListStatus) -> TaskRecord {
    TaskRecord {
        id: id.into(),
        subject: subject.into(),
        description: String::new(),
        active_form: None,
        owner: None,
        status,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: None,
    }
}

fn todo(content: &str, status: &str) -> TodoRecord {
    TodoRecord {
        content: content.into(),
        status: status.into(),
        active_form: String::new(),
    }
}

fn lines_text(lines: &[ActivityLine]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.text.as_ref()).collect::<String>())
        .collect()
}

#[test]
fn empty_state_appends_nothing() {
    let state = AppState::new();
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    assert!(out.is_empty(), "expected no lines, got {out:?}");
}

#[test]
fn v2_wins_when_both_have_content() {
    let mut state = AppState::new();
    state
        .session
        .plan_tasks
        .push(task("1", "v2 task", TaskListStatus::InProgress));
    state
        .session
        .todos_by_agent
        .insert("agent".into(), vec![todo("v1 todo", "in_progress")]);
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    assert!(joined.contains("v2 task"), "V2 row missing: {joined}");
    assert!(!joined.contains("v1 todo"), "V1 row leaked: {joined}");
}

#[test]
fn v1_renders_when_no_v2() {
    let mut state = AppState::new();
    state
        .session
        .todos_by_agent
        .insert("agent".into(), vec![todo("v1 todo", "pending")]);
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    assert!(joined.contains("v1 todo"), "V1 row missing: {joined}");
}

#[test]
fn v2_sort_orders_in_progress_first() {
    let mut state = AppState::new();
    state
        .session
        .plan_tasks
        .push(task("1", "done thing", TaskListStatus::Completed));
    state
        .session
        .plan_tasks
        .push(task("2", "later thing", TaskListStatus::Pending));
    state
        .session
        .plan_tasks
        .push(task("3", "active thing", TaskListStatus::InProgress));
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    // active should appear before pending, which appears before done.
    let pos_active = joined.find("active thing").expect("active missing");
    let pos_pending = joined.find("later thing").expect("pending missing");
    let pos_done = joined.find("done thing").expect("done missing");
    assert!(pos_active < pos_pending);
    assert!(pos_pending < pos_done);
}

#[test]
fn v2_uses_ts_glyphs() {
    let mut state = AppState::new();
    state
        .session
        .plan_tasks
        .push(task("1", "a", TaskListStatus::InProgress));
    state
        .session
        .plan_tasks
        .push(task("2", "b", TaskListStatus::Pending));
    state
        .session
        .plan_tasks
        .push(task("3", "c", TaskListStatus::Completed));
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    assert!(joined.contains("✔"), "tick glyph missing: {joined}");
    assert!(joined.contains("◼"), "in-progress glyph missing: {joined}");
    assert!(joined.contains("☐"), "pending glyph missing: {joined}");
}

/// Pinned-time AppState helper. `T0_MS` is an arbitrary epoch
/// anchor; the test then stamps completion timestamps relative to it
/// so `append_lines` reads the same `now` from the injected clock.
const T0_MS: i64 = 1_700_000_000_000;

fn state_at(now_ms: i64) -> AppState {
    AppState::with_clock(coco_tui_ui::clock::MockClock::arc(now_ms))
}

#[test]
fn recently_completed_lifts_above_pending() {
    let mut state = state_at(T0_MS);
    state
        .session
        .plan_tasks
        .push(task("1", "pending thing", TaskListStatus::Pending));
    state
        .session
        .plan_tasks
        .push(task("2", "just done", TaskListStatus::Completed));
    // Stamp completion 5 seconds ago — well within the 30s window.
    state
        .ui
        .ephemeral
        .task_completion_timestamps
        .insert("2".into(), T0_MS - 5_000);
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    let pos_done = joined.find("just done").expect("recent done missing");
    let pos_pending = joined.find("pending thing").expect("pending missing");
    assert!(
        pos_done < pos_pending,
        "recently-completed should sort above pending: {joined}"
    );
}

#[test]
fn old_completed_sorts_below_pending() {
    let mut state = state_at(T0_MS);
    state
        .session
        .plan_tasks
        .push(task("1", "pending thing", TaskListStatus::Pending));
    state
        .session
        .plan_tasks
        .push(task("2", "long ago done", TaskListStatus::Completed));
    // 45 seconds ago — outside the 30s window.
    state
        .ui
        .ephemeral
        .task_completion_timestamps
        .insert("2".into(), T0_MS - 45_000);
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    let pos_pending = joined.find("pending thing").expect("pending missing");
    let pos_done = joined.find("long ago done").expect("old done missing");
    assert!(
        pos_pending < pos_done,
        "older completed should trail pending: {joined}"
    );
}

#[test]
fn all_completed_hides_after_5_seconds() {
    let mut state = state_at(T0_MS);
    state
        .session
        .plan_tasks
        .push(task("1", "done a", TaskListStatus::Completed));
    state
        .session
        .plan_tasks
        .push(task("2", "done b", TaskListStatus::Completed));
    // Anchor set 6 seconds ago — past the 5s window.
    state.ui.ephemeral.tasks_all_completed_since_ms = Some(T0_MS - 6_000);

    let mut out = Vec::new();
    append_lines(&state, &mut out);
    assert!(out.is_empty(), "expected hidden panel, got {out:?}");
}

#[test]
fn all_completed_visible_within_5_seconds() {
    let mut state = state_at(T0_MS);
    state
        .session
        .plan_tasks
        .push(task("1", "fresh done", TaskListStatus::Completed));
    // Anchor 2 seconds ago — still in the celebration window.
    state.ui.ephemeral.tasks_all_completed_since_ms = Some(T0_MS - 2_000);

    let mut out = Vec::new();
    append_lines(&state, &mut out);
    assert!(!out.is_empty(), "panel should still render within window");
    let joined = lines_text(&out).join("\n");
    assert!(joined.contains("fresh done"));
}

#[test]
fn boundary_30s_recently_completed_excluded() {
    // Exact 30 000 ms gap should NOT count as recent (TS `<` not `<=`).
    let mut state = state_at(T0_MS);
    state
        .session
        .plan_tasks
        .push(task("1", "pending thing", TaskListStatus::Pending));
    state
        .session
        .plan_tasks
        .push(task("2", "right at the line", TaskListStatus::Completed));
    state
        .ui
        .ephemeral
        .task_completion_timestamps
        .insert("2".into(), T0_MS - 30_000);
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    let pos_pending = joined.find("pending thing").expect("pending missing");
    let pos_done = joined.find("right at the line").expect("missing");
    assert!(
        pos_pending < pos_done,
        "completion exactly 30s old must trail pending — `<` boundary"
    );
}

#[test]
fn v1_sort_orders_in_progress_first() {
    let mut state = AppState::new();
    state.session.todos_by_agent.insert(
        "agent".into(),
        vec![
            todo("done task", "completed"),
            todo("later task", "pending"),
            todo("active task", "in_progress"),
        ],
    );
    let mut out = Vec::new();
    append_lines(&state, &mut out);
    let joined = lines_text(&out).join("\n");
    let pos_active = joined.find("active task").expect("active missing");
    let pos_pending = joined.find("later task").expect("pending missing");
    assert!(pos_active < pos_pending);
}
