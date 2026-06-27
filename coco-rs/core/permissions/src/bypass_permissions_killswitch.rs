//! Bypass-permissions killswitch.
//!
//! An emergency override that forcibly disables `BypassPermissions` mode
//! regardless of settings. Two activation vectors:
//!
//! 1. **Env var** `COCO_PERMISSIONS_DISABLE_BYPASS=1` — local operator override
//!    for CI, shared workstations, or security-sensitive runs.
//! 2. **Enterprise policy** — `settings.json` field
//!    `bypassPermissionsKillswitch: true` at the policy scope
//!    (managed-settings). Overrides user/project/local settings.
//!
//! When active, any attempt to transition into `BypassPermissions` (via
//! slash command, CLI flag, or SDK control) is rejected with a clear
//! reason and the session stays in its current mode.

use coco_config::env;
use coco_types::PermissionMode;

/// Env var name that activates the killswitch when set to a truthy value.
/// Single source of truth is the typed [`coco_config::EnvKey`] registry.
pub const KILLSWITCH_ENV: &str = coco_config::EnvKey::CocoPermissionsDisableBypass.as_str();

/// Settings key (policy scope) for the killswitch flag.
pub const KILLSWITCH_SETTING_KEY: &str = "bypassPermissionsKillswitch";

/// Outcome of a killswitch check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillswitchCheck {
    /// Killswitch not engaged; the transition may proceed.
    Allowed,
    /// Killswitch engaged via env var.
    BlockedByEnv,
    /// Killswitch engaged via managed-settings policy.
    BlockedByPolicy,
}

impl KillswitchCheck {
    /// Whether the check allows the mode transition.
    pub fn is_allowed(&self) -> bool {
        matches!(self, KillswitchCheck::Allowed)
    }

    /// Human-readable reason suitable for surfacing in tool_use_error or
    /// as the message argument to `PermissionDecision::Deny`. Returns
    /// `None` when allowed.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            KillswitchCheck::Allowed => None,
            KillswitchCheck::BlockedByEnv => Some(
                "BypassPermissions disabled by operator override \
                 (COCO_PERMISSIONS_DISABLE_BYPASS environment variable).",
            ),
            KillswitchCheck::BlockedByPolicy => Some(
                "BypassPermissions disabled by enterprise policy \
                 (bypassPermissionsKillswitch managed setting).",
            ),
        }
    }
}

/// Check whether the killswitch is engaged.
///
/// `policy_flag` is the managed-settings `bypassPermissionsKillswitch`
/// value at the policy scope (`None` when unset).
pub fn check(policy_flag: Option<bool>) -> KillswitchCheck {
    check_with_env_truthy(policy_flag, env::is_env_truthy(KILLSWITCH_ENV))
}

fn check_with_env_truthy(policy_flag: Option<bool>, env_truthy: bool) -> KillswitchCheck {
    if env_truthy {
        return KillswitchCheck::BlockedByEnv;
    }
    if policy_flag == Some(true) {
        return KillswitchCheck::BlockedByPolicy;
    }
    KillswitchCheck::Allowed
}

/// Check whether the requested transition is permitted under the killswitch.
///
/// The killswitch only gates `BypassPermissions`; transitions into any
/// other mode (including `AcceptEdits` and `Plan`) proceed normally.
pub fn check_transition(requested: PermissionMode, policy_flag: Option<bool>) -> KillswitchCheck {
    check_transition_with_env_truthy(requested, policy_flag, env::is_env_truthy(KILLSWITCH_ENV))
}

fn check_transition_with_env_truthy(
    requested: PermissionMode,
    policy_flag: Option<bool>,
    env_truthy: bool,
) -> KillswitchCheck {
    if requested != PermissionMode::BypassPermissions {
        return KillswitchCheck::Allowed;
    }
    check_with_env_truthy(policy_flag, env_truthy)
}

/// Compute the session-wide "bypass permissions available" capability.
///
/// ```text
/// isBypassPermissionsModeAvailable =
///   (permissionMode === 'bypassPermissions' || allowDangerouslySkipPermissions)
///   && !growthBookDisable && !settingsDisable
/// ```
///
/// Note that the check keys on the **resolved** permission mode
/// (after CLI-flag + settings merge + killswitch downgrade), NOT on
/// the raw `--dangerously-skip-permissions` CLI bool. That's why the
/// first parameter here is `starts_in_bypass_mode`: callers resolve the
/// initial mode first (including any `--permission-mode` override or
/// `settings.permissions.default_mode`), then pass
/// `resolved_mode == BypassPermissions` here.
///
/// Returns `true` iff the session is authorized to transition into
/// `BypassPermissions` — either because the resolved initial mode *is*
/// bypass, or because `--allow-dangerously-skip-permissions` unlocked
/// it, AND no policy killswitch is engaged.
///
/// Static for the lifetime of the session; threaded into
/// `QueryEngineConfig::bypass_permissions_available`,
/// `ToolPermissionContext::bypass_available`, and TUI
/// `SessionState::bypass_permissions_available`.
pub fn compute_bypass_capability(
    starts_in_bypass_mode: bool,
    allow_dangerously_skip_permissions: bool,
    policy_flag: Option<bool>,
) -> bool {
    compute_bypass_capability_with_env_truthy(
        starts_in_bypass_mode,
        allow_dangerously_skip_permissions,
        policy_flag,
        env::is_env_truthy(KILLSWITCH_ENV),
    )
}

fn compute_bypass_capability_with_env_truthy(
    starts_in_bypass_mode: bool,
    allow_dangerously_skip_permissions: bool,
    policy_flag: Option<bool>,
    env_truthy: bool,
) -> bool {
    if !(starts_in_bypass_mode || allow_dangerously_skip_permissions) {
        return false;
    }
    check_with_env_truthy(policy_flag, env_truthy).is_allowed()
}

/// Outcome of resolving the session's initial permission mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialPermissionMode {
    /// The mode the session should start in.
    pub mode: PermissionMode,
    /// Human-readable notification if a candidate was downgraded
    /// (e.g. bypass blocked by killswitch → fell through to the next
    /// candidate or to `Default`). `None` when no downgrade occurred.
    pub notification: Option<String>,
}

/// Resolve the session's initial permission mode by walking an ordered
/// list of candidates and skipping `BypassPermissions` when the
/// killswitch is engaged.
///
/// The three candidate slots (in priority order) are:
///  1. `--dangerously-skip-permissions` → `BypassPermissions`
///  2. `--permission-mode <mode>` → whatever the flag parses to
///  3. `settings.permissions.default_mode` → user-configured default
///
/// The function walks these in order. The first *non-blocked* candidate
/// wins. `BypassPermissions` is blocked by the killswitch (env var
/// `COCO_PERMISSIONS_DISABLE_BYPASS` or `policy_flag == Some(true)`); every
/// other mode is always allowed. If every candidate is blocked or the
/// list is empty, the resolved mode is `Default`.
///
/// The `notification` field surfaces any downgrade reason so the caller
/// can print it to stderr — used by the TUI banner on startup.
///
/// Note: `--allow-dangerously-skip-permissions` does NOT appear in the
/// candidate list — it affects only the capability gate, not the
/// initial mode. See [`compute_bypass_capability`] for that path.
pub fn resolve_initial_permission_mode(
    dangerously_skip_permissions: bool,
    permission_mode_cli: Option<PermissionMode>,
    settings_default_mode: Option<PermissionMode>,
    policy_flag: Option<bool>,
) -> InitialPermissionMode {
    resolve_initial_permission_mode_with_env_truthy(
        dangerously_skip_permissions,
        permission_mode_cli,
        settings_default_mode,
        policy_flag,
        env::is_env_truthy(KILLSWITCH_ENV),
    )
}

fn resolve_initial_permission_mode_with_env_truthy(
    dangerously_skip_permissions: bool,
    permission_mode_cli: Option<PermissionMode>,
    settings_default_mode: Option<PermissionMode>,
    policy_flag: Option<bool>,
    env_truthy: bool,
) -> InitialPermissionMode {
    let killswitch = check_with_env_truthy(policy_flag, env_truthy);

    let mut candidates: Vec<PermissionMode> = Vec::with_capacity(3);
    if dangerously_skip_permissions {
        candidates.push(PermissionMode::BypassPermissions);
    }
    if let Some(m) = permission_mode_cli {
        candidates.push(m);
    }
    if let Some(m) = settings_default_mode {
        candidates.push(m);
    }

    let mut notification: Option<String> = None;
    for candidate in candidates {
        if candidate == PermissionMode::BypassPermissions && !killswitch.is_allowed() {
            // Set the notification on the first skip and keep walking.
            // Subsequent skips of the same mode don't re-overwrite
            // (the first reason is the most specific).
            if notification.is_none()
                && let Some(reason) = killswitch.reason()
            {
                notification = Some(reason.to_string());
            }
            continue;
        }
        return InitialPermissionMode {
            mode: candidate,
            notification,
        };
    }

    InitialPermissionMode {
        mode: PermissionMode::Default,
        notification,
    }
}

#[cfg(test)]
#[path = "bypass_permissions_killswitch.test.rs"]
mod tests;
