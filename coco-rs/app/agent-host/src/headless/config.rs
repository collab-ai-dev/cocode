use super::*;

// ─── RuntimeConfig construction ──────────────────────────────────────

/// Derive `RuntimeOverrides` from the parsed CLI flags.
/// Validates numeric flags up-front so a non-positive value can't
/// silently propagate down to the budget tracker (where `<=0` would
/// trigger immediate "budget exhausted" and short-circuit every LLM
/// call to an empty response).
pub fn cli_runtime_overrides(cli: &AgentHostOptions) -> Result<coco_config::RuntimeOverrides> {
    use coco_types::ProviderModelSelection;

    let mut overrides = coco_config::RuntimeOverrides::default();
    if let Some(raw) = cli.models_main.as_deref() {
        overrides.model_override = Some(
            ProviderModelSelection::from_slash_str(raw)
                .map_err(|e| anyhow::anyhow!("--models.main: {e}"))?,
        );
    }
    if let Some(mode) = cli.permission_mode.as_deref()
        && let Ok(pm) = serde_json::from_value::<coco_types::PermissionMode>(
            serde_json::Value::String(mode.to_string()),
        )
    {
        overrides.permission_mode_override = Some(pm);
    }
    overrides.fallback_model_overrides = cli
        .fallback_model
        .iter()
        .map(|raw| {
            ProviderModelSelection::from_slash_str(raw)
                .map_err(|e| anyhow::anyhow!("--fallback-model: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;
    overrides.event_hub_url_override = cli.event_hub_url.clone();
    if let Some(max_tokens) = cli.max_tokens
        && max_tokens <= 0
    {
        anyhow::bail!(
            "--max-tokens must be > 0 (got {max_tokens}); a non-positive value short-circuits \
             the budget tracker and produces empty responses"
        );
    }
    if let Some(max_turns) = cli.max_turns
        && max_turns < 1
    {
        anyhow::bail!(
            "--max-turns must be >= 1 (got {max_turns}); 0 or negative would prevent the \
             agent loop from executing any turn"
        );
    }
    Ok(overrides)
}

/// Build a `RuntimeConfig` honoring CLI-level overrides.
pub fn build_runtime_config_for_cli(
    cli: &AgentHostOptions,
    cwd: &Path,
) -> Result<coco_config::RuntimeConfig> {
    let roots = crate::paths::settings_roots_for_cwd(cwd);
    build_runtime_config_for_cli_with_roots(cli, roots.project_root(), roots.local_root())
}

/// Build a `RuntimeConfig` honoring CLI-level overrides with split
/// project/local settings roots.
pub fn build_runtime_config_for_cli_with_roots(
    cli: &AgentHostOptions,
    project_root: &Path,
    local_root: &Path,
) -> Result<coco_config::RuntimeConfig> {
    let mut builder = coco_config::RuntimeConfigBuilder::from_process(local_root)
        .with_overrides(cli_runtime_overrides(cli)?)
        .with_settings_roots(project_root, local_root)
        .with_setting_sources(cli.setting_sources.clone());
    if let Some(path) = cli.settings.as_deref() {
        builder = builder.with_flag_settings(path);
    }
    Ok(builder.build()?)
}

/// Build a `RuntimeConfig` with a live `RuntimeReloader` so settings.json edits
/// hot-reload (sandbox, …) on the AppServer / headless paths too — not just the TUI.
/// Falls back to a one-shot static build when the reloader can't spawn (e.g.
/// outside a Tokio runtime). Callers must keep the returned reloader alive for
/// the session and ask the session handle to install its sandbox reload
/// supervisor after `SessionRuntime::build`.
pub fn build_runtime_config_with_reloader(
    cli: &AgentHostOptions,
    cwd: &Path,
) -> Result<(
    Option<coco_config_reload::RuntimeReloader>,
    coco_config::RuntimeConfig,
)> {
    let roots = crate::paths::settings_roots_for_cwd(cwd);
    build_runtime_config_with_reloader_roots(cli, roots.project_root(), roots.local_root())
}

/// Build a `RuntimeConfig` with hot-reload and split project/local settings
/// roots.
pub fn build_runtime_config_with_reloader_roots(
    cli: &AgentHostOptions,
    project_root: &Path,
    local_root: &Path,
) -> Result<(
    Option<coco_config_reload::RuntimeReloader>,
    coco_config::RuntimeConfig,
)> {
    let reload_opts = coco_config_reload::ReloadOptions::new(local_root.to_path_buf())
        .with_settings_roots(project_root, local_root)
        .with_overrides(cli_runtime_overrides(cli)?)
        .with_setting_sources(cli.setting_sources.clone());
    let reload_opts = if let Some(path) = cli.settings.as_deref() {
        reload_opts.with_flag_settings(path)
    } else {
        reload_opts
    };
    match coco_config_reload::RuntimeReloader::spawn(reload_opts) {
        Ok(reloader) => {
            let snapshot = reloader.current();
            Ok((Some(reloader), Arc::unwrap_or_clone(snapshot)))
        }
        Err(e) => {
            tracing::warn!(error = %e, "config hot-reload disabled; using one-shot build");
            Ok((
                None,
                build_runtime_config_for_cli_with_roots(cli, project_root, local_root)?,
            ))
        }
    }
}
