use super::*;

// ─── Permission resolution ───────────────────────────────────────────

/// Resolved startup permission state.
pub struct StartupPermissionState {
    pub mode: coco_types::PermissionMode,
    pub bypass_available: bool,
    /// Whether the classifier-backed `Auto` mode can be cycled into / set.
    /// Default-on, gated only by the `auto_mode.disabled` settings opt-out.
    pub auto_available: bool,
    pub notification: Option<String>,
}

/// Resolve the session's initial `PermissionMode` and the bypass capability.
///
/// Takes the source-tracked settings rather than the merged snapshot: bypass
/// posture must ignore the `Project` layer, and that distinction only exists
/// before the layers are flattened. See
/// [`coco_config::SettingsWithSource::startup_permission_mode`].
pub fn resolve_startup_permission_state(
    cli: &AgentHostOptions,
    settings: &coco_config::SettingsWithSource,
) -> Result<StartupPermissionState> {
    use coco_types::PermissionMode;

    let policy_flag = Some(settings.disable_bypass_mode_enabled());

    let permission_mode_cli = cli.permission_mode.as_deref().and_then(|raw| {
        match serde_json::from_value::<PermissionMode>(serde_json::json!(raw)) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("warning: invalid --permission-mode {raw:?}: {e}; ignoring");
                None
            }
        }
    });

    let resolved = coco_permissions::resolve_initial_permission_mode(
        cli.dangerously_skip_permissions,
        permission_mode_cli,
        settings.startup_permission_mode(),
        policy_flag,
    );
    let mode = resolved.mode;

    let bypass_available = coco_permissions::compute_bypass_capability(
        mode == PermissionMode::BypassPermissions,
        cli.allow_dangerously_skip_permissions,
        policy_flag,
    );

    let auto_available = coco_permissions::compute_auto_mode_capability(
        settings
            .merged
            .auto_mode
            .as_ref()
            .is_some_and(|c| c.disabled),
    );

    let requesting_bypass =
        mode == PermissionMode::BypassPermissions || cli.allow_dangerously_skip_permissions;
    enforce_dangerous_skip_safety(requesting_bypass)?;

    Ok(StartupPermissionState {
        mode,
        bypass_available,
        auto_available,
        notification: resolved.notification,
    })
}

fn enforce_dangerous_skip_safety(requesting_bypass: bool) -> Result<()> {
    if !requesting_bypass {
        return Ok(());
    }
    if is_running_as_root() && !is_sandboxed_env() {
        return Err(anyhow::anyhow!(
            "Bypass permissions refuses to run as root/sudo outside a \
             sandbox. Set IS_SANDBOX=1 (or run under bubblewrap) if you \
             know what you're doing."
        ));
    }
    Ok(())
}

/// True when the process runs with effective root privileges (euid 0) — actual
/// root or under `sudo`. Checks the *effective* uid so `sudo coco` is also
/// caught (the prior env-name heuristic — `SUDO_USER`/`USER == root` — was a
/// fragile, spoofable proxy for this). Non-Unix has no uid → false.
fn is_running_as_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` is an always-succeeds libc call — no preconditions,
        // no arguments, no memory effects.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn is_sandboxed_env() -> bool {
    let truthy = |var: &str| -> bool {
        std::env::var(var)
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    };
    truthy("IS_SANDBOX") || coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBubblewrap)
}
