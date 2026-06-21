//! Permission mode transitions — cycling modes via Shift+Tab.
//!
//! Mode cycle: default → acceptEdits → plan → [bypass] → [auto] → default

use coco_types::PermissionMode;
use coco_types::ToolAppState;
use coco_types::ToolPermissionContext;

use crate::auto_mode_state::AutoModeState;
use crate::dangerous_rules::restore_dangerous_rules;
use crate::dangerous_rules::strip_dangerous_rules;

/// Determine the next permission mode when cycling (Shift+Tab).
///
/// Thin wrapper around [`PermissionMode::next_in_cycle`] that reads the
/// bypass gate flag off the [`ToolPermissionContext`]. Kept for call sites
/// that already hold a full context. Plan/auto availability are passed in
/// because they are session-level capabilities (`plan_mode` feature gate /
/// classifier availability), not permission-context fields.
pub fn get_next_permission_mode(
    context: &ToolPermissionContext,
    is_plan_available: bool,
    is_auto_available: bool,
) -> PermissionMode {
    context.mode.next_in_cycle(
        is_plan_available,
        context.bypass_available,
        is_auto_available,
    )
}

/// Resolve the initial permission mode from CLI flags and settings.
///
/// Priority: explicit CLI flag > settings default > Default
pub fn resolve_predefined_mode(
    cli_mode: Option<PermissionMode>,
    settings_default: Option<PermissionMode>,
) -> PermissionMode {
    cli_mode
        .or(settings_default)
        .unwrap_or(PermissionMode::Default)
}

/// Compute a subagent's effective permission mode from the parent's mode
/// and the agent-definition-level request.
///
/// Rules:
/// - If the parent is in a **trust mode** (`BypassPermissions`,
///   `AcceptEdits`, or `Auto`), the child **always inherits** the parent's
///   mode — the agent's own declaration is ignored. The idea: if the user
///   granted parent broad permissions, don't let a nested agent quietly
///   downgrade and re-ask.
/// - Otherwise, if the agent definition declares a mode
///   (`agent_requested`), that wins.
/// - Otherwise, inherit parent.
pub fn resolve_subagent_mode(
    parent: PermissionMode,
    agent_requested: Option<PermissionMode>,
) -> PermissionMode {
    let trust = matches!(
        parent,
        PermissionMode::BypassPermissions | PermissionMode::AcceptEdits | PermissionMode::Auto
    );
    if trust {
        parent
    } else {
        agent_requested.unwrap_or(parent)
    }
}

/// Prepare context for a mode transition (simple — no auto-mode state).
///
/// When entering Plan mode, stash the current mode so we can restore later.
/// When leaving Plan mode, restore the pre-plan mode.
pub fn transition_context(
    mut context: ToolPermissionContext,
    from: PermissionMode,
    to: PermissionMode,
) -> ToolPermissionContext {
    // Entering plan mode: stash current mode
    if to == PermissionMode::Plan && from != PermissionMode::Plan {
        context.pre_plan_mode = Some(from);
    }

    // Leaving plan mode: restore (but don't clear — caller manages lifecycle)
    context.mode = to;
    context
}

/// Apply the Auto-mode side-effects of a mode transition directly to a
/// shared [`ToolAppState`]. Called by the TUI and SDK mode-toggle
/// handlers so the engine's live state reflects the same stash/clear
/// logic that `transition_context_with_auto` applies to a per-batch
/// context.
///
/// Scope: this helper only handles the Auto **exit** clear (leaving Auto
/// drops the `stripped_dangerous_rules` stash so the next rebuild doesn't
/// carry it into a non-Auto mode). Auto **entry** snapshots+strips the
/// dangerous allow rules in
/// [`apply_permission_mode_transition_to_app_state`] (it has the live
/// `allow_rules`); the evaluator-facing strip is applied at
/// `ToolContextFactory::build` keyed on live mode==Auto.
///
/// Returns `true` if the stash was modified (useful for logging /
/// regression tests).
pub fn apply_auto_transition_to_app_state(
    guard: &mut coco_types::ToolAppState,
    from: PermissionMode,
    to: PermissionMode,
) -> bool {
    let from_auto = from == PermissionMode::Auto;
    let to_auto = to == PermissionMode::Auto;
    if from_auto && !to_auto && guard.permissions.stripped_dangerous_rules.is_some() {
        guard.permissions.stripped_dangerous_rules = None;
        return true;
    }
    false
}

/// Apply a full permission-mode transition to shared app state.
///
/// This is the app-state-shaped equivalent of the permission mode transition
/// function, plus the plan-mode entry timestamp used by plan-mode
/// lifecycle consumers.
///
/// Call this from external mode switchers (TUI Shift+Tab, SDK control,
/// bridge control, `/plan`) and model-driven plan entry. It centralizes
/// the plan-mode latches so every entry path preserves the same invariants:
///
/// - non-Plan → Plan stashes `pre_plan_mode`, clears a stale exit banner,
///   and stamps `plan_mode_entry_ms`;
/// - Plan → non-Plan marks the session as having exited plan mode, schedules
///   the one-shot exit banner, clears classifier stash if plan mode had Auto
///   active, and clears `pre_plan_mode`;
/// - non-Auto → Auto snapshots+strips dangerous allow rules from the supplied
///   live `allow_rules` into `stripped_dangerous_rules` (restore provenance +
///   the `## Exited Auto Mode` banner proxy). The evaluator-facing strip is
///   applied separately at `ToolContextFactory::build` keyed on live
///   mode==Auto, so the security guard does not depend on this stash;
/// - Auto → non-Auto clears `stripped_dangerous_rules` via the existing
///   auto-boundary helper.
///
/// **"Plan had Auto active" proxy.** coco-rs has no mid-plan
/// Auto-deactivation path (no mid-plan Auto deactivation), so
/// `pre_plan_mode == Some(Auto) ||
/// stripped_dangerous_rules.is_some()` is a faithful proxy here — and it
/// is the *same* proxy `ExitPlanModeTool::execute` uses, keeping the
/// external and tool-driven exit paths consistent. If a
/// `transitionPlanAutoMode` equivalent is ever ported, both call sites
/// must switch to threading `AutoModeState` instead.
///
/// Returns `true` when any field was changed.
pub fn apply_permission_mode_transition_to_app_state(
    guard: &mut ToolAppState,
    from: PermissionMode,
    to: PermissionMode,
    live_allow_rules: &coco_types::PermissionRulesBySource,
) -> bool {
    let before_permission_mode = guard.permissions.mode;
    let before_pre_plan_mode = guard.permissions.pre_plan_mode;
    let before_needs_plan_exit = guard.needs_plan_mode_exit_attachment;
    let before_needs_auto_exit = guard.needs_auto_mode_exit_attachment;
    let before_has_exited = guard.has_exited_plan_mode;
    let before_plan_entry = guard.plan_mode_entry_ms;
    let before_stripped = guard.permissions.stripped_dangerous_rules.is_some();

    if from != to {
        if to == PermissionMode::Plan && from != PermissionMode::Plan {
            guard.permissions.pre_plan_mode = Some(from);
            guard.needs_plan_mode_exit_attachment = false;
            guard.plan_mode_entry_ms = Some(current_epoch_ms());
        }

        if from == PermissionMode::Plan && to != PermissionMode::Plan {
            let plan_used_auto = guard.permissions.pre_plan_mode == Some(PermissionMode::Auto)
                || guard.permissions.stripped_dangerous_rules.is_some();
            let restoring_to_auto = to == PermissionMode::Auto;
            guard.permissions.pre_plan_mode = None;
            guard.has_exited_plan_mode = true;
            guard.needs_plan_mode_exit_attachment = true;
            if plan_used_auto && !restoring_to_auto {
                guard.permissions.stripped_dangerous_rules = None;
                guard.needs_auto_mode_exit_attachment = true;
            }
        }
    }

    // Auto entry: snapshot + strip dangerous allow rules into the stash.
    // The evaluator-facing strip happens at context-build time
    // (ToolContextFactory::build keyed on live mode == Auto); this stash
    // exists for restore provenance + the `## Exited Auto Mode` banner proxy.
    // `is_ant_user=false` is the external-user path.
    if to == PermissionMode::Auto
        && from != PermissionMode::Auto
        && guard.permissions.stripped_dangerous_rules.is_none()
    {
        let mut snapshot = live_allow_rules.clone();
        if let Some(stripped) = crate::dangerous_rules::strip_dangerous_allow_rules(
            &mut snapshot,
            /*is_ant_user*/ false,
        ) {
            guard.permissions.stripped_dangerous_rules = Some(stripped);
        }
    }

    guard.permissions.mode = Some(to);
    let auto_modified = apply_auto_transition_to_app_state(guard, from, to);

    auto_modified
        || before_permission_mode != guard.permissions.mode
        || before_pre_plan_mode != guard.permissions.pre_plan_mode
        || before_needs_plan_exit != guard.needs_plan_mode_exit_attachment
        || before_needs_auto_exit != guard.needs_auto_mode_exit_attachment
        || before_has_exited != guard.has_exited_plan_mode
        || before_plan_entry != guard.plan_mode_entry_ms
        || before_stripped != guard.permissions.stripped_dangerous_rules.is_some()
}

fn current_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Prepare context for a mode transition with auto-mode state management.
///
/// Handles dangerous rule stripping on auto-mode entry and restoration on exit.
/// Also manages the `AutoModeState.active` flag.
///
/// - Computes `from_uses_classifier` and `to_uses_classifier` as unified booleans
/// - Entering classifier: activate auto + strip dangerous rules
/// - Leaving classifier: deactivate auto + restore dangerous rules
/// - Plan mode entry handled specially (stash pre_plan_mode)
pub fn transition_context_with_auto(
    mut context: ToolPermissionContext,
    from: PermissionMode,
    to: PermissionMode,
    auto_state: &AutoModeState,
    is_ant_user: bool,
) -> ToolPermissionContext {
    if from == to {
        return context;
    }

    // ── Plan mode entry ──
    // Must happen before the classifier transition logic because
    // entering plan from auto requires stashing pre_plan_mode first.
    if to == PermissionMode::Plan && from != PermissionMode::Plan {
        context.pre_plan_mode = Some(from);
    }

    // ── Classifier transition ──
    // "Uses classifier" means: auto mode, OR plan mode with auto active.
    let from_uses_classifier =
        from == PermissionMode::Auto || (from == PermissionMode::Plan && auto_state.is_active());
    // Plan entry with auto is handled by prepareContextForPlanMode (not here).
    let to_uses_classifier = to == PermissionMode::Auto;

    if to_uses_classifier && !from_uses_classifier {
        // Entering classifier territory: activate auto, strip dangerous rules.
        auto_state.set_active(true);
        strip_dangerous_rules(&mut context, is_ant_user);
    } else if from_uses_classifier && !to_uses_classifier {
        // Leaving classifier territory: deactivate auto, restore dangerous rules.
        auto_state.set_active(false);
        restore_dangerous_rules(&mut context);
    }

    // ── Plan mode exit ──
    if from == PermissionMode::Plan && to != PermissionMode::Plan {
        context.pre_plan_mode = None;
    }

    context.mode = to;
    context
}

#[cfg(test)]
#[path = "mode_transition.test.rs"]
mod tests;
