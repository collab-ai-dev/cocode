//! Engine-facing helpers that map [`ToolAppState`] → [`GeneratorContextBuilder`]
//! fields.
//!
//! These are thin composition helpers — the engine still needs to supply
//! per-turn values the `ToolAppState` doesn't carry (tool list, model info,
//! token counts, date latch, pre-computed turn counters). Their purpose is
//! to package the state→builder mapping in one place so every call site
//! doesn't re-derive the same booleans.
//!
//! Pairing with [`crate::turn_counting`]:
//! - `turn_counting`: history → scalar counters
//! - `context_builder`: app state → builder field assignments
//! - Caller: combines the two and any remaining per-turn inputs

use coco_types::PermissionMode;
use coco_types::ToolAppState;

use crate::generator::GeneratorContextBuilder;

/// Apply plan-mode fields from `app_state` onto the builder.
///
/// Sets:
///
/// - `is_plan_mode` — true iff the live permission mode is `Plan`.
/// - `is_plan_reentry` — forwards `ToolAppState::has_exited_plan_mode`.
///   The engine clears that flag after the orchestrator consumes a Reentry.
/// - `needs_plan_mode_exit_attachment` — forwards the one-shot flag.
/// - `needs_auto_mode_exit_attachment` — forwards the one-shot flag.
/// - `is_auto_mode` — `mode == Auto` **or** (`mode == Plan` and
///   `is_auto_classifier_active`). The classifier flag is a session-scoped
///   `AutoModeState.is_active()` read by the engine before building the
///   `TurnReminderInput`; keeping it as a parameter here avoids pulling
///   `core/permissions` into this helper's dependency graph.
///
/// Does NOT set:
///
/// - `plan_file_path` / `plan_exists` — these are filesystem lookups; the
///   engine already resolves them via `coco_context::get_plan_file_path`.
/// - `is_sub_agent` / `agent_id` — trivially available from engine config.
/// - `plan_workflow` / `phase4_variant` / agent counts — from settings.json.
/// - `todos` / `plan_tasks` — direct copies the engine does itself since
///   the builder takes them by value.
///
/// Returns the builder for chaining with further setters.
pub fn apply_app_state<'a>(
    mut builder: GeneratorContextBuilder<'a>,
    app_state: &ToolAppState,
    fallback_permission_mode: PermissionMode,
    is_auto_classifier_active: bool,
) -> GeneratorContextBuilder<'a> {
    let mode = app_state
        .permissions
        .mode
        .unwrap_or(fallback_permission_mode);
    let in_auto = mode == PermissionMode::Auto;
    let in_plan_with_auto = mode == PermissionMode::Plan && is_auto_classifier_active;
    builder = builder
        .is_plan_mode(mode == PermissionMode::Plan)
        .is_auto_mode(in_auto || in_plan_with_auto)
        .is_plan_reentry(app_state.has_exited_plan_mode)
        .needs_plan_mode_exit_attachment(app_state.needs_plan_mode_exit_attachment)
        .pending_plan_mode_exit_outcome(app_state.pending_plan_mode_exit_outcome)
        .needs_auto_mode_exit_attachment(app_state.needs_auto_mode_exit_attachment);
    builder = builder.plan_tasks(app_state.plan_tasks.clone());
    builder
}

/// Apply the per-agent todo-list snapshot from `app_state`.
///
/// Todos are keyed by `agentId ?? sessionId`; callers pass the
/// already-resolved key.
pub fn apply_todos_for_key<'a>(
    builder: GeneratorContextBuilder<'a>,
    app_state: &ToolAppState,
    todo_key: &str,
) -> GeneratorContextBuilder<'a> {
    let todos = app_state
        .todos_by_agent
        .get(todo_key)
        .cloned()
        .unwrap_or_default();
    builder.todos(todos)
}

#[cfg(test)]
#[path = "context_builder.test.rs"]
mod tests;
