use std::path::PathBuf;
use std::sync::Arc;

use tracing::warn;

use coco_config::RuntimeConfig;

/// Construct an `Arc<SandboxState>` for the active session, or return `None`
/// when sandbox is disabled / unavailable.
/// Returns `Ok(None)` when:
/// - `Feature::Sandbox` is off, or
/// - mode is `FullAccess`, or
/// - bootstrap gates fail AND `sandbox.fail_if_unavailable` is `false`
/// (commands will run unsandboxed; user gets a startup banner).
/// Returns `Err` when bootstrap gates fail AND
/// `sandbox.fail_if_unavailable` is `true` - the caller propagates this
/// so coco exits before the REPL starts.
pub(crate) async fn build_sandbox_state(
    runtime_config: &RuntimeConfig,
    cwd: &std::path::Path,
) -> anyhow::Result<Option<Arc<coco_sandbox::SandboxState>>> {
    use coco_sandbox::adapter::AdapterInputs;

    if !runtime_config
        .features
        .enabled(coco_types::Feature::Sandbox)
    {
        return Ok(None);
    }

    let mode = runtime_config.sandbox.mode;
    if matches!(mode, coco_types::SandboxMode::FullAccess) {
        return Ok(None);
    }

    // Mark `enabled = true` because reaching this point already implies the
    // feature gate passed and the user requested an enforcing mode.
    let mut sandbox_settings = runtime_config.sandbox.clone();
    sandbox_settings.enabled = true;

    let gate = coco_sandbox::check_enable_gates(&sandbox_settings);
    if !matches!(gate, coco_sandbox::EnableCheckResult::Enabled) {
        // Surface a startup banner via `sandbox_unavailable_reason` so the
        // user understands *why* sandboxing is degraded. When
        // `fail_if_unavailable` is set, this is a hard error.
        let missing_deps: Vec<String> = match &gate {
            coco_sandbox::EnableCheckResult::DisabledByMissingDeps { missing } => missing.clone(),
            _ => Vec::new(),
        };
        let reason = coco_sandbox::sandbox_unavailable_reason(
            &sandbox_settings,
            coco_sandbox::current_platform_supported(),
            sandbox_settings.is_platform_enabled(),
            &missing_deps,
        );

        if sandbox_settings.fail_if_unavailable {
            let detail = reason.unwrap_or_else(|| format!("sandbox bootstrap failed: {gate:?}"));
            return Err(anyhow::anyhow!(
                "sandbox.fail_if_unavailable is set but sandbox cannot start: {detail}"
            ));
        }

        if let Some(banner) = reason {
            // stderr so the message survives any TUI redirection.
            eprintln!("[coco] sandbox unavailable: {banner}");
            warn!(?gate, banner, "sandbox enabled but runtime cannot start");
        } else {
            warn!(?gate, "sandbox enabled but runtime cannot start");
        }
        return Ok(None);
    }

    let settings_root = runtime_config
        .paths
        .project_dir
        .clone()
        .unwrap_or_else(|| cwd.to_path_buf());

    let permission_allow_rules: Vec<String> =
        runtime_config.settings.merged.permissions.allow.clone();
    let permission_deny_rules: Vec<String> =
        runtime_config.settings.merged.permissions.deny.clone();
    let additional_directories: Vec<PathBuf> = runtime_config
        .settings
        .merged
        .permissions
        .additional_directories
        .iter()
        .map(PathBuf::from)
        .collect();

    let coco_temp_dir = std::env::temp_dir().join("coco");
    let worktree = coco_sandbox::detect_worktree_main_repo(cwd);

    // Per-source rule plumbing - drives the `allow_managed_*_only`
    // gates. The adapter needs source provenance because the merged
    // `SandboxSettings` collapses every layer; only allow rules need
    // sourcing (deny rules apply uniformly regardless of source).
    // The sandbox adapter only consumes allow-source provenance today
    // (deny rules apply uniformly regardless of source). `_ask` is
    // ignored here; the ask map is consumed at the engine config layer
    // via `permission_rule_loader::typed_permission_rules`.
    let (sourced_allow_rules, _sourced_deny_rules, _sourced_ask_rules) =
        runtime_config.settings.sourced_permission_rules();
    let sourced_fs_allow_read = runtime_config.settings.sourced_filesystem_allow_read();
    let sourced_sandbox_credentials = runtime_config
        .settings
        .sourced_sandbox_credentials(&settings_root);

    // Deny writes to every settings source so a sandboxed command can't edit
    // its own permission rules (or disable the sandbox), plus the `project config dir/`
    // command/agent definitions.
    let settings_files = sandbox_settings_deny_paths(&settings_root);

    let inputs = AdapterInputs {
        settings: &sandbox_settings,
        mode,
        settings_root: &settings_root,
        original_cwd: cwd,
        current_cwd: cwd,
        permission_allow_rules: &permission_allow_rules,
        permission_deny_rules: &permission_deny_rules,
        additional_directories: &additional_directories,
        coco_temp_dir: &coco_temp_dir,
        settings_files: &settings_files,
        worktree_main_repo: worktree.as_deref(),
        sourced_permission_allow_rules: Some(&sourced_allow_rules),
        sourced_filesystem_allow_read: Some(&sourced_fs_allow_read),
        sourced_sandbox_credentials: sourced_sandbox_credentials.as_ref(),
    };
    let out = coco_sandbox::build_runtime_config(inputs);
    // `allow_network == false` means network is isolated (the safe default once
    // the sandbox is enabled, unless the coarse `sandbox.allow_network` toggle
    // is set, or the mode is `FullAccess`).
    let network_isolated = !out.config.allow_network;

    let platform = coco_sandbox::platform::create_platform();
    let state = match mode {
        coco_types::SandboxMode::ExternalSandbox => {
            coco_sandbox::SandboxState::external(out.enforcement, out.settings, out.config)
        }
        _ => coco_sandbox::SandboxState::new(out.enforcement, out.settings, out.config, platform),
    };
    let state = Arc::new(state);

    if network_isolated {
        // Start the egress proxy so the `DomainFilter` enforces deny-by-default
        // per-domain filtering. macOS reaches the proxy over loopback directly;
        // Linux runs inside a `--unshare-net` namespace and needs the netns
        // socat bridge (requires `socat`) to forward egress through the proxy.
        // On any failure / missing socat the posture stays fail-closed (network
        // blocked) - never unrestricted egress. The coarse
        // `sandbox.allow_network` toggle opts back into full network.
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = state.start_network_proxy().await {
                warn!(error = %e, "sandbox egress proxy failed to start; network is blocked this session");
            }
        }
        #[cfg(target_os = "linux")]
        {
            let tag = state.session_tag().to_string();
            match coco_sandbox::deps::socat_path() {
                Some(socat) => {
                    if let Err(e) = state.start_network_proxy_with_bridge(socat, &tag).await {
                        warn!(error = %e, "sandbox network bridge failed to start; network is blocked this session");
                    }
                }
                None => warn!(
                    "socat not found - install socat (apt install socat) to enable \
                     per-domain network filtering; network is blocked this session"
                ),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            warn!(
                "sandbox network isolation blocks ALL egress on this platform - \
                 per-domain filtering is unsupported; set sandbox.allow_network = true \
                 to allow network"
            );
        }
    }

    Ok(Some(state))
}

/// Settings / definition files denied write inside the sandbox, so a sandboxed
/// command cannot rewrite its own permission rules, disable the sandbox, or
/// inject agents.
/// MUST be the single source for both the bootstrap (`build_sandbox_state`) and
/// the hot-reload (`sandbox_reload::reapply_sandbox`) paths - passing `&[]` on
/// reload silently re-opened the self-permission escape after the first
/// settings change.
pub(crate) fn sandbox_settings_deny_paths(settings_root: &std::path::Path) -> Vec<PathBuf> {
    use coco_config::global_config;
    let managed = global_config::managed_settings_path();
    let mut paths = vec![
        global_config::user_settings_path(),
        global_config::project_settings_path(settings_root),
        global_config::local_settings_path(settings_root),
        managed.clone(),
        global_config::global_config_path(),
        settings_root
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("agents"),
    ];
    // Managed drop-in directory (`managed-settings.d`) next to the managed
    // settings file - deny the whole drop-in dir, not just the .json.
    if let Some(dir) = managed.parent() {
        paths.push(dir.join("managed-settings.d"));
    }
    paths
}
