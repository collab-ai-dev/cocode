use std::collections::HashMap;

use coco_types::PermissionMode;
use coco_types::ToolPermissionContext;

use super::*;

fn make_context(mode: PermissionMode, bypass: bool) -> ToolPermissionContext {
    ToolPermissionContext {
        mode,
        additional_dirs: HashMap::new(),
        permission_rule_source_roots: HashMap::new(),
        allow_rules: HashMap::new(),
        deny_rules: HashMap::new(),
        ask_rules: HashMap::new(),
        bypass_available: bypass,
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        session_plan_file: None,
    }
}

fn dangerous_agent_allow_rules() -> coco_types::PermissionRulesBySource {
    let mut live = coco_types::PermissionRulesBySource::new();
    live.entry(coco_types::PermissionRuleSource::UserSettings)
        .or_default()
        .push(coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::UserSettings,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: "Agent".into(),
                rule_content: None,
            },
        });
    live
}

fn plan_auto_options() -> PlanModeAutoOptions {
    PlanModeAutoOptions {
        use_auto_mode_during_plan: true,
        auto_mode_available: true,
    }
}

// ── get_next_permission_mode ──

#[test]
fn test_default_to_accept_edits() {
    let ctx = make_context(PermissionMode::Default, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::AcceptEdits
    );
}

#[test]
fn test_accept_edits_to_plan() {
    let ctx = make_context(PermissionMode::AcceptEdits, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::Plan
    );
}

#[test]
fn test_accept_edits_skips_plan_when_unavailable() {
    // plan_mode feature off → AcceptEdits jumps over Plan to the next
    // available rung (Default here, since bypass/auto are both off).
    let ctx = make_context(PermissionMode::AcceptEdits, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ false, false),
        PermissionMode::Default
    );
}

#[test]
fn test_plan_to_bypass_when_available() {
    let ctx = make_context(PermissionMode::Plan, true);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::BypassPermissions
    );
}

#[test]
fn test_plan_to_auto_when_bypass_unavailable() {
    let ctx = make_context(PermissionMode::Plan, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, true),
        PermissionMode::Auto
    );
}

#[test]
fn test_plan_to_default_when_nothing_available() {
    let ctx = make_context(PermissionMode::Plan, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::Default
    );
}

#[test]
fn test_bypass_to_auto_when_available() {
    let ctx = make_context(PermissionMode::BypassPermissions, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, true),
        PermissionMode::Auto
    );
}

#[test]
fn test_bypass_to_default() {
    let ctx = make_context(PermissionMode::BypassPermissions, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::Default
    );
}

#[test]
fn test_auto_to_default() {
    let ctx = make_context(PermissionMode::Auto, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::Default
    );
}

#[test]
fn test_dont_ask_to_default() {
    let ctx = make_context(PermissionMode::DontAsk, false);
    assert_eq!(
        get_next_permission_mode(&ctx, /*plan*/ true, false),
        PermissionMode::Default
    );
}

// ── resolve_predefined_mode ──

#[test]
fn test_resolve_cli_wins() {
    assert_eq!(
        resolve_predefined_mode(Some(PermissionMode::Auto), Some(PermissionMode::Plan)),
        PermissionMode::Auto
    );
}

#[test]
fn test_resolve_settings_fallback() {
    assert_eq!(
        resolve_predefined_mode(None, Some(PermissionMode::Plan)),
        PermissionMode::Plan
    );
}

#[test]
fn test_resolve_default_fallback() {
    assert_eq!(resolve_predefined_mode(None, None), PermissionMode::Default);
}

// ── transition_context ──

#[test]
fn test_transition_stashes_pre_plan() {
    let ctx = make_context(PermissionMode::Default, false);
    let result = transition_context(ctx, PermissionMode::Default, PermissionMode::Plan);
    assert_eq!(result.mode, PermissionMode::Plan);
    assert_eq!(result.pre_plan_mode, Some(PermissionMode::Default));
}

#[test]
fn test_transition_from_accept_edits_to_plan() {
    let ctx = make_context(PermissionMode::AcceptEdits, false);
    let result = transition_context(ctx, PermissionMode::AcceptEdits, PermissionMode::Plan);
    assert_eq!(result.pre_plan_mode, Some(PermissionMode::AcceptEdits));
}

#[test]
fn test_transition_plan_to_plan_no_stash() {
    let mut ctx = make_context(PermissionMode::Plan, false);
    ctx.pre_plan_mode = Some(PermissionMode::Default);
    let result = transition_context(ctx, PermissionMode::Plan, PermissionMode::Plan);
    // Should NOT overwrite the stash
    assert_eq!(result.pre_plan_mode, Some(PermissionMode::Default));
}

// ── resolve_subagent_mode ──

#[test]
fn subagent_inherits_parent_default_when_no_request() {
    assert_eq!(
        resolve_subagent_mode(PermissionMode::Default, None),
        PermissionMode::Default
    );
}

#[test]
fn subagent_inherits_parent_plan_when_no_request() {
    assert_eq!(
        resolve_subagent_mode(PermissionMode::Plan, None),
        PermissionMode::Plan
    );
}

#[test]
fn subagent_request_wins_over_non_trust_parent() {
    // Parent Default, agent requests Plan → child uses Plan.
    assert_eq!(
        resolve_subagent_mode(PermissionMode::Default, Some(PermissionMode::Plan)),
        PermissionMode::Plan
    );
}

#[test]
fn subagent_request_ignored_when_parent_accept_edits() {
    // Trust mode — child always inherits parent, declaration ignored.
    assert_eq!(
        resolve_subagent_mode(PermissionMode::AcceptEdits, Some(PermissionMode::Plan)),
        PermissionMode::AcceptEdits
    );
}

#[test]
fn subagent_request_ignored_when_parent_bypass() {
    assert_eq!(
        resolve_subagent_mode(
            PermissionMode::BypassPermissions,
            Some(PermissionMode::Plan),
        ),
        PermissionMode::BypassPermissions
    );
}

#[test]
fn subagent_request_ignored_when_parent_auto() {
    assert_eq!(
        resolve_subagent_mode(PermissionMode::Auto, Some(PermissionMode::Default)),
        PermissionMode::Auto
    );
}

// ── apply_auto_transition_to_app_state ──

#[test]
fn auto_transition_clears_stash_on_leaving_auto() {
    // Restores dangerous permissions (clears the stash) when the classifier exits.
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            stripped_dangerous_rules: Some(coco_types::PermissionRulesBySource::default()),
            ..Default::default()
        },
        ..Default::default()
    };
    let modified = apply_auto_transition_to_app_state(
        &mut state,
        PermissionMode::Auto,
        PermissionMode::Default,
    );
    assert!(modified, "Auto→Default with stash should report modified");
    assert!(state.permissions.stripped_dangerous_rules.is_none());
}

#[test]
fn auto_transition_noop_when_entering_auto() {
    // Entering Auto: full rule-stashing is deferred (needs central
    // rules store). Helper returns false and leaves stash alone.
    let mut state = coco_types::ToolAppState::default();
    let modified = apply_auto_transition_to_app_state(
        &mut state,
        PermissionMode::Default,
        PermissionMode::Auto,
    );
    assert!(!modified);
    assert!(state.permissions.stripped_dangerous_rules.is_none());
}

#[test]
fn auto_transition_noop_when_no_stash_to_clear() {
    // Leaving Auto but no stash present (e.g. Auto was purely a mode
    // label with no rules stashed) → no-op.
    let mut state = coco_types::ToolAppState::default();
    let modified = apply_auto_transition_to_app_state(
        &mut state,
        PermissionMode::Auto,
        PermissionMode::Default,
    );
    assert!(!modified);
}

#[test]
fn auto_transition_noop_for_non_auto_boundary() {
    // Default → Plan shouldn't touch the stash either way.
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            stripped_dangerous_rules: Some(coco_types::PermissionRulesBySource::default()),
            ..Default::default()
        },
        ..Default::default()
    };
    let modified = apply_auto_transition_to_app_state(
        &mut state,
        PermissionMode::Default,
        PermissionMode::Plan,
    );
    assert!(!modified);
    // Stash preserved — non-Auto transitions don't manage it.
    assert!(state.permissions.stripped_dangerous_rules.is_some());
}

// ── apply_permission_mode_transition_to_app_state ──

#[test]
fn app_state_transition_enter_plan_stashes_previous_mode_and_timestamp() {
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::AcceptEdits),
            ..Default::default()
        },
        has_exited_plan_mode: true,
        needs_plan_mode_exit_attachment: true,
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
        &coco_types::PermissionRulesBySource::new(),
        PlanModeAutoOptions::default(),
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Plan));
    assert_eq!(
        state.permissions.pre_plan_mode,
        Some(PermissionMode::AcceptEdits)
    );
    assert!(state.has_exited_plan_mode);
    assert!(!state.needs_plan_mode_exit_attachment);
    assert!(state.plan_mode_entry_ms.unwrap_or_default() > 0);
}

#[test]
fn app_state_transition_default_to_auto_stashes_dangerous_allow_rules() {
    // P1: entering Auto snapshots+strips dangerous classifier-bypassing allow
    // rules into the stash (provenance for the exit banner / restore). The
    // evaluator-facing strip is additionally applied at context-build time.
    let live = dangerous_agent_allow_rules();

    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Default),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Default,
        PermissionMode::Auto,
        &live,
        PlanModeAutoOptions::default(),
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Auto));
    assert!(
        state.permissions.stripped_dangerous_rules.is_some(),
        "entering Auto must stash the dangerous Agent allow rule"
    );
}

#[test]
fn app_state_transition_plan_to_default_sets_exit_latches_and_clears_stash() {
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::Auto),
            stripped_dangerous_rules: Some(coco_types::PermissionRulesBySource::default()),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Plan,
        PermissionMode::Default,
        &coco_types::PermissionRulesBySource::new(),
        PlanModeAutoOptions::default(),
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Default));
    assert_eq!(state.permissions.pre_plan_mode, None);
    assert!(state.has_exited_plan_mode);
    assert!(state.needs_plan_mode_exit_attachment);
    assert!(state.needs_auto_mode_exit_attachment);
    assert!(
        state.permissions.stripped_dangerous_rules.is_none(),
        "Plan→Default after Auto-backed plan mode must clear classifier stash"
    );
}

#[test]
fn app_state_transition_plan_to_plan_preserves_existing_entry_timestamp() {
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::Default),
            ..Default::default()
        },
        plan_mode_entry_ms: Some(42),
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Plan,
        PermissionMode::Plan,
        &coco_types::PermissionRulesBySource::new(),
        PlanModeAutoOptions::default(),
    );

    assert!(!modified);
    assert_eq!(
        state.permissions.pre_plan_mode,
        Some(PermissionMode::Default)
    );
    assert_eq!(state.plan_mode_entry_ms, Some(42));
}

#[test]
fn app_state_transition_plan_to_auto_preserves_classifier_stash() {
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::Auto),
            stripped_dangerous_rules: Some(coco_types::PermissionRulesBySource::default()),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Plan,
        PermissionMode::Auto,
        &coco_types::PermissionRulesBySource::new(),
        PlanModeAutoOptions::default(),
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Auto));
    assert_eq!(state.permissions.pre_plan_mode, None);
    assert!(state.has_exited_plan_mode);
    assert!(state.needs_plan_mode_exit_attachment);
    assert!(!state.needs_auto_mode_exit_attachment);
    assert!(
        state.permissions.stripped_dangerous_rules.is_some(),
        "Plan→Auto keeps classifier stash because Auto remains active"
    );
}

#[test]
fn app_state_transition_default_to_plan_with_auto_stashes_dangerous_allow_rules() {
    let live = dangerous_agent_allow_rules();
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Default),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Default,
        PermissionMode::Plan,
        &live,
        plan_auto_options(),
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Plan));
    assert_eq!(
        state.permissions.pre_plan_mode,
        Some(PermissionMode::Default)
    );
    assert!(
        state.permissions.stripped_dangerous_rules.is_some(),
        "Plan mode with auto semantics must stash dangerous allow rules"
    );
    assert!(!state.needs_auto_mode_exit_attachment);
}

#[test]
fn app_state_transition_bypass_to_plan_with_auto_does_not_stash() {
    let live = dangerous_agent_allow_rules();
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::BypassPermissions),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::BypassPermissions,
        PermissionMode::Plan,
        &live,
        plan_auto_options(),
    );

    assert!(modified);
    assert_eq!(
        state.permissions.pre_plan_mode,
        Some(PermissionMode::BypassPermissions)
    );
    assert!(
        state.permissions.stripped_dangerous_rules.is_none(),
        "Bypass-origin plan mode must not activate auto semantics"
    );
}

#[test]
fn app_state_transition_auto_to_plan_with_auto_preserves_stash() {
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Auto),
            stripped_dangerous_rules: Some(dangerous_agent_allow_rules()),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Auto,
        PermissionMode::Plan,
        &coco_types::PermissionRulesBySource::new(),
        plan_auto_options(),
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Plan));
    assert_eq!(state.permissions.pre_plan_mode, Some(PermissionMode::Auto));
    assert!(
        state.permissions.stripped_dangerous_rules.is_some(),
        "Auto-origin plan mode keeps auto's dangerous-rule stash"
    );
    assert!(!state.needs_auto_mode_exit_attachment);
}

#[test]
fn app_state_transition_auto_to_plan_without_auto_clears_stash_and_sets_exit_banner() {
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Auto),
            stripped_dangerous_rules: Some(dangerous_agent_allow_rules()),
            ..Default::default()
        },
        ..Default::default()
    };

    let modified = apply_permission_mode_transition_to_app_state(
        &mut state,
        PermissionMode::Auto,
        PermissionMode::Plan,
        &coco_types::PermissionRulesBySource::new(),
        PlanModeAutoOptions {
            use_auto_mode_during_plan: false,
            auto_mode_available: true,
        },
    );

    assert!(modified);
    assert_eq!(state.permissions.mode, Some(PermissionMode::Plan));
    assert_eq!(state.permissions.pre_plan_mode, Some(PermissionMode::Auto));
    assert!(
        state.permissions.stripped_dangerous_rules.is_none(),
        "Auto-origin plan mode without auto semantics exits classifier territory"
    );
    assert!(state.needs_auto_mode_exit_attachment);
}

#[test]
fn reconcile_plan_auto_mode_activates_auto_mid_plan_and_stashes_dangerous_rules() {
    let auto_state = AutoModeState::new();
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::Default),
            ..Default::default()
        },
        needs_auto_mode_exit_attachment: true,
        ..Default::default()
    };

    let changed = reconcile_plan_auto_mode_in_app_state(
        &mut state,
        &dangerous_agent_allow_rules(),
        plan_auto_options(),
        &auto_state,
    );

    assert!(changed);
    assert!(auto_state.is_active());
    assert!(!state.needs_auto_mode_exit_attachment);
    assert!(
        state.permissions.stripped_dangerous_rules.is_some(),
        "activation should keep restore provenance for dangerous allow rules"
    );
}

#[test]
fn reconcile_plan_auto_mode_deactivates_auto_mid_plan_and_clears_stale_stash() {
    let auto_state = AutoModeState::new();
    auto_state.set_active(true);
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::Default),
            stripped_dangerous_rules: Some(dangerous_agent_allow_rules()),
            ..Default::default()
        },
        ..Default::default()
    };

    let changed = reconcile_plan_auto_mode_in_app_state(
        &mut state,
        &dangerous_agent_allow_rules(),
        PlanModeAutoOptions {
            use_auto_mode_during_plan: false,
            auto_mode_available: true,
        },
        &auto_state,
    );

    assert!(changed);
    assert!(!auto_state.is_active());
    assert!(state.needs_auto_mode_exit_attachment);
    assert!(
        state.permissions.stripped_dangerous_rules.is_none(),
        "deactivation should leave stale stripped rules unavailable as an auto-active proxy"
    );
}

#[test]
fn reconcile_plan_auto_mode_never_activates_from_bypass_origin() {
    let auto_state = AutoModeState::new();
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: Some(PermissionMode::BypassPermissions),
            ..Default::default()
        },
        ..Default::default()
    };

    let changed = reconcile_plan_auto_mode_in_app_state(
        &mut state,
        &dangerous_agent_allow_rules(),
        plan_auto_options(),
        &auto_state,
    );

    assert!(!changed);
    assert!(!auto_state.is_active());
    assert!(state.permissions.stripped_dangerous_rules.is_none());
}

#[test]
fn reconcile_plan_auto_mode_never_activates_without_pre_plan_mode() {
    let auto_state = AutoModeState::new();
    let mut state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(PermissionMode::Plan),
            pre_plan_mode: None,
            ..Default::default()
        },
        ..Default::default()
    };

    let changed = reconcile_plan_auto_mode_in_app_state(
        &mut state,
        &dangerous_agent_allow_rules(),
        plan_auto_options(),
        &auto_state,
    );

    assert!(
        !changed,
        "2.1.193 transitionPlanAutoMode returns unchanged without prePlanMode"
    );
    assert!(!auto_state.is_active());
    assert!(state.permissions.stripped_dangerous_rules.is_none());
    assert!(!state.needs_auto_mode_exit_attachment);
}
