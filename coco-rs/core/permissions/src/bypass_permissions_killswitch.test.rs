use coco_types::PermissionMode;

use super::KillswitchCheck;
use super::check;
use super::check_transition_with_env_truthy;
use super::check_with_env_truthy;
use super::compute_bypass_capability_with_env_truthy;
use super::resolve_initial_permission_mode_with_env_truthy;

#[test]
fn killswitch_full_behavior() {
    // Env-sensitive behavior is tested through pure helpers. Do not
    // mutate process env here: cargo runs tests concurrently, and
    // std::env is global to the whole test process.
    assert_eq!(check_with_env_truthy(None, false), KillswitchCheck::Allowed);
    assert_eq!(
        check_with_env_truthy(Some(false), false),
        KillswitchCheck::Allowed
    );

    let policy = check_with_env_truthy(Some(true), false);
    assert_eq!(policy, KillswitchCheck::BlockedByPolicy);
    assert!(!policy.is_allowed());
    assert!(policy.reason().unwrap().contains("enterprise policy"));

    assert_eq!(
        check_with_env_truthy(None, true),
        KillswitchCheck::BlockedByEnv
    );
    assert_eq!(
        check_with_env_truthy(Some(true), true),
        KillswitchCheck::BlockedByEnv
    );

    // — Transition gate: killswitch only blocks `BypassPermissions`.
    assert_eq!(
        check_transition_with_env_truthy(PermissionMode::Default, Some(true), false),
        KillswitchCheck::Allowed,
    );
    assert_eq!(
        check_transition_with_env_truthy(PermissionMode::AcceptEdits, Some(true), false),
        KillswitchCheck::Allowed,
    );
    assert_eq!(
        check_transition_with_env_truthy(PermissionMode::Plan, Some(true), false),
        KillswitchCheck::Allowed,
    );
    assert_eq!(
        check_transition_with_env_truthy(PermissionMode::BypassPermissions, Some(true), false),
        KillswitchCheck::BlockedByPolicy,
    );
    assert_eq!(
        check_transition_with_env_truthy(PermissionMode::BypassPermissions, None, false),
        KillswitchCheck::Allowed,
    );
    assert_eq!(
        check_transition_with_env_truthy(PermissionMode::BypassPermissions, None, true),
        KillswitchCheck::BlockedByEnv,
    );
}

#[test]
fn check_reads_real_env_and_parser_without_leaking_it() {
    let previous = std::env::var(super::KILLSWITCH_ENV).ok();

    unsafe {
        std::env::remove_var(super::KILLSWITCH_ENV);
    }
    assert_eq!(check(None), KillswitchCheck::Allowed);

    for value in ["1", "true", "True", "YES", "on"] {
        unsafe {
            std::env::set_var(super::KILLSWITCH_ENV, value);
        }
        assert_eq!(
            check(None),
            KillswitchCheck::BlockedByEnv,
            "expected `{value}` to be truthy"
        );
    }

    for value in ["0", "false", "no", "off", ""] {
        unsafe {
            std::env::set_var(super::KILLSWITCH_ENV, value);
        }
        assert_eq!(
            check(None),
            KillswitchCheck::Allowed,
            "expected `{value}` to be falsy"
        );
    }

    match previous {
        Some(value) => unsafe {
            std::env::set_var(super::KILLSWITCH_ENV, value);
        },
        None => unsafe {
            std::env::remove_var(super::KILLSWITCH_ENV);
        },
    }
}

/// Exhaustive matrix for bypass capability: every combination of the three
/// inputs verified so future refactors can't silently break the gate.
#[test]
fn compute_bypass_capability_matrix() {
    // Neither trigger → never available.
    assert!(!compute_bypass_capability_with_env_truthy(
        false, false, None, false
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        false,
        false,
        Some(false),
        false
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        false,
        false,
        Some(true),
        false
    ));

    // `starts_in_bypass_mode` alone → available iff killswitch allows.
    assert!(compute_bypass_capability_with_env_truthy(
        true, false, None, false
    ));
    assert!(compute_bypass_capability_with_env_truthy(
        true,
        false,
        Some(false),
        false
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        true,
        false,
        Some(true),
        false
    )); // policy blocks

    // `allow_dangerously_skip_permissions` alone → same rule.
    assert!(compute_bypass_capability_with_env_truthy(
        false, true, None, false
    ));
    assert!(compute_bypass_capability_with_env_truthy(
        false,
        true,
        Some(false),
        false
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        false,
        true,
        Some(true),
        false
    )); // policy blocks

    // Both triggers → still gated by killswitch (policy is a hard deny).
    assert!(compute_bypass_capability_with_env_truthy(
        true, true, None, false
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        true,
        true,
        Some(true),
        false
    ));

    // Env killswitch overrides everything.
    assert!(!compute_bypass_capability_with_env_truthy(
        true, false, None, true
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        false, true, None, true
    ));
    assert!(!compute_bypass_capability_with_env_truthy(
        true, true, None, true
    ));
}

/// Walk-and-skip behavior for initial permission mode resolution.
/// When the killswitch blocks `BypassPermissions`, the walk must fall
/// through to the next candidate, not collapse straight to `Default`.
#[test]
fn resolve_initial_permission_mode_matrix() {
    // — No candidates → Default, no notification.
    let r = resolve_initial_permission_mode_with_env_truthy(false, None, None, None, false);
    assert_eq!(r.mode, PermissionMode::Default);
    assert!(r.notification.is_none());

    // — Only --dangerously-skip, killswitch off → Bypass, no notification.
    let r = resolve_initial_permission_mode_with_env_truthy(true, None, None, None, false);
    assert_eq!(r.mode, PermissionMode::BypassPermissions);
    assert!(r.notification.is_none());

    // — Only --dangerously-skip, killswitch on → Default, notification set.
    let r = resolve_initial_permission_mode_with_env_truthy(true, None, None, Some(true), false);
    assert_eq!(r.mode, PermissionMode::Default);
    assert!(r.notification.is_some());

    // — --dangerously-skip + --permission-mode acceptEdits, killswitch on
    //   → AcceptEdits (walk-and-skip). This is the case my old code
    //   collapsed to Default instead of falling through.
    let r = resolve_initial_permission_mode_with_env_truthy(
        true,
        Some(PermissionMode::AcceptEdits),
        None,
        Some(true),
        false,
    );
    assert_eq!(r.mode, PermissionMode::AcceptEdits);
    assert!(r.notification.is_some()); // downgrade still surfaced

    // — --dangerously-skip + settings.default_mode = acceptEdits,
    //   killswitch on → AcceptEdits (second fallback in the walk).
    let r = resolve_initial_permission_mode_with_env_truthy(
        true,
        None,
        Some(PermissionMode::AcceptEdits),
        Some(true),
        false,
    );
    assert_eq!(r.mode, PermissionMode::AcceptEdits);
    assert!(r.notification.is_some());

    // — --dangerously-skip + --permission-mode + settings default, all
    //   with killswitch on, --permission-mode also bypassPermissions.
    //   The walk skips bypass twice, falls to settings default.
    let r = resolve_initial_permission_mode_with_env_truthy(
        true,
        Some(PermissionMode::BypassPermissions),
        Some(PermissionMode::Plan),
        Some(true),
        false,
    );
    assert_eq!(r.mode, PermissionMode::Plan);

    // — All three are bypass, killswitch on → Default (walk exhausts).
    let r = resolve_initial_permission_mode_with_env_truthy(
        true,
        Some(PermissionMode::BypassPermissions),
        Some(PermissionMode::BypassPermissions),
        Some(true),
        false,
    );
    assert_eq!(r.mode, PermissionMode::Default);
    assert!(r.notification.is_some());

    // — --permission-mode alone wins over settings default (first in order).
    let r = resolve_initial_permission_mode_with_env_truthy(
        false,
        Some(PermissionMode::Plan),
        Some(PermissionMode::AcceptEdits),
        None,
        false,
    );
    assert_eq!(r.mode, PermissionMode::Plan);

    // — Only settings default, no CLI → use it.
    let r = resolve_initial_permission_mode_with_env_truthy(
        false,
        None,
        Some(PermissionMode::AcceptEdits),
        None,
        false,
    );
    assert_eq!(r.mode, PermissionMode::AcceptEdits);

    // — Settings default is bypass + killswitch → Default (no other
    //   candidate to fall through to).
    let r = resolve_initial_permission_mode_with_env_truthy(
        false,
        None,
        Some(PermissionMode::BypassPermissions),
        Some(true),
        false,
    );
    assert_eq!(r.mode, PermissionMode::Default);

    // — Env killswitch also blocks (same walk behavior).
    let r = resolve_initial_permission_mode_with_env_truthy(
        true,
        Some(PermissionMode::AcceptEdits),
        None,
        None,
        true,
    );
    assert_eq!(r.mode, PermissionMode::AcceptEdits);
    assert!(r.notification.is_some());
}
