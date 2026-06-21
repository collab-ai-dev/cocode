use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

/// Per-user global config. Separate from Settings.
/// Stored at `~/.coco.json` (or `$COCO_CONFIG_DIR/global.json` when set).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    pub projects: HashMap<String, ProjectConfig>,
    pub session_costs: HashMap<String, SessionCostState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion: Option<CompanionConfig>,
    /// Has the user completed onboarding?
    #[serde(default)]
    pub has_completed_onboarding: bool,
    /// Cached org-level settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_settings_cache: Option<serde_json::Value>,
    /// Plugin-hint recommendation state (show-once record + opt-out flag).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_code_hints: Option<ClaudeCodeHintsState>,
}

/// Persisted plugin-hint state.
///
/// `plugin` is a show-once record per plugin slug — a plugin is prompted
/// for at most once ever. `disabled` is set when the user picks "don't show
/// plugin installation hints again".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeCodeHintsState {
    /// Plugin slugs (`name@marketplace`) already surfaced in a prompt.
    pub plugin: Vec<String>,
    /// User opted out of all plugin-installation hints.
    pub disabled: bool,
}

/// Per-project config within GlobalConfig.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub costs: Option<SessionCostState>,
    /// Set true once the project has completed onboarding (CLAUDE.md
    /// exists). Used by `maybeMarkProjectOnboardingComplete` to
    /// short-circuit subsequent /init invocations.
    #[serde(default)]
    pub has_completed_project_onboarding: bool,
}

/// Session cost tracking state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionCostState {
    pub total_cost_usd: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

/// Companion pet config (buddy).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Get the config home directory.
///
/// Respects `COCO_CONFIG_DIR` env var, defaults to `~/.coco`. Delegates to
/// `coco_utils_common::find_coco_home` so MCP and every other consumer
/// agree on one implementation (empty-string filtering, cross-platform
/// `dirs::home_dir()` fallback, last-resort cwd fallback).
pub fn config_home() -> PathBuf {
    coco_utils_common::find_coco_home()
}

/// Get the global config file path.
///
/// Priority:
/// 1. If `COCO_CONFIG_DIR` is set, put `global.json` inside that dir so
///    the whole coco workspace (settings + global state + sessions)
///    moves as a unit. Useful for sandboxed / per-project setups.
/// 2. Otherwise, fall back to `~/.coco.json`, a sibling of `~/.coco/`.
pub fn global_config_path() -> PathBuf {
    if let Some(custom) =
        std::env::var_os(coco_utils_common::COCO_CONFIG_DIR_ENV).filter(|s| !s.is_empty())
    {
        return PathBuf::from(custom).join("global.json");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".coco.json")
}

/// Load global config from disk.
pub fn load_global_config() -> crate::Result<GlobalConfig> {
    let path = global_config_path();
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let contents = std::fs::read_to_string(&path)?;
    let config: GlobalConfig = crate::jsonc::from_str(&contents)?;
    Ok(config)
}

/// Write global config to disk.
pub fn write_global_config(config: &GlobalConfig) -> crate::Result<()> {
    let path = global_config_path();
    write_global_config_at_path(&path, config)
}

fn write_global_config_at_path(path: &Path, config: &GlobalConfig) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let updated = if contents.trim().is_empty() {
        serde_json::to_string_pretty(config)?
    } else {
        let value = serde_json::to_value(config)?;
        crate::jsonc::update_value_preserving_format(&contents, value)?
    };
    std::fs::write(path, updated)?;
    Ok(())
}

/// Get the user settings path.
pub fn user_settings_path() -> PathBuf {
    config_home().join("settings.json")
}

/// Path to `~/.coco/providers.json` — provider catalog sibling of
/// `settings.json`. See `docs/coco-rs/multi-provider-plan.md` §4.
pub fn providers_catalog_path() -> PathBuf {
    config_home().join("providers.json")
}

/// Path to `~/.coco/models.json` — provider-agnostic ModelInfo
/// catalog sibling of `settings.json`.
pub fn models_catalog_path() -> PathBuf {
    config_home().join("models.json")
}

/// Get the project settings path (`<cwd>/.coco/settings.json`, checked in).
pub fn project_settings_path(cwd: &Path) -> PathBuf {
    cwd.join(".coco/settings.json")
}

/// Get the local (gitignored) settings path (`<cwd>/.coco/settings.local.json`).
pub fn local_settings_path(cwd: &Path) -> PathBuf {
    cwd.join(".coco/settings.local.json")
}

/// Set `key` to `value` in `~/.coco/settings.json`.
///
/// Creates the file and parent directory as needed. Used by slash-command
/// handlers that need to persist a single setting without round-tripping
/// through the full `Settings` deserialize/serialize cycle.
///
/// `key` may be dotted (`sandbox.mode`) — intermediate objects are
/// created if absent. Existing siblings are preserved. Returns the
/// path that was written so callers can show it to the user. Invalid
/// existing JSON is returned as an error and left untouched.
///
/// **Reload semantics**: writes to disk; the live runtime keeps the
/// pre-existing in-memory `Settings` until the user starts a new
/// session (or the SettingsWatcher debounce fires and re-loads).
/// Slash-command settings writes are observed by the next session,
/// not the current one.
pub fn write_user_setting(key: &str, value: serde_json::Value) -> crate::Result<PathBuf> {
    let path = user_settings_path();
    write_user_setting_to_path(&path, key, value)
}

fn write_user_setting_to_path(
    path: &Path,
    key: &str,
    value: serde_json::Value,
) -> crate::Result<PathBuf> {
    write_user_setting_at_path(path, key, value)?;
    Ok(path.to_path_buf())
}

fn write_user_setting_at_path(
    path: &Path,
    key: &str,
    value: serde_json::Value,
) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let updated = crate::jsonc::set_dotted_value_preserving_format(&contents, key, value)?;
    crate::settings::parse_settings(&updated)?;
    std::fs::write(path, updated)?;
    Ok(())
}

/// Best-effort mark the project at `cwd` as having completed onboarding
/// when a `CLAUDE.md` exists at the project root. No-op when the flag
/// is already set, when no `CLAUDE.md` is present, or when the global
/// config can't be read/written. Errors are swallowed because
/// onboarding state is opportunistic — losing it doesn't impact
/// correctness, only the cosmetic onboarding banner.
///
/// Called once per `/init` invocation.
pub fn maybe_mark_project_onboarding_complete(cwd: &Path) {
    let key = cwd.to_string_lossy().to_string();
    let mut config = match load_global_config() {
        Ok(c) => c,
        Err(_) => return,
    };
    if let Some(p) = config.projects.get(&key)
        && p.has_completed_project_onboarding
    {
        return;
    }
    if !cwd.join("CLAUDE.md").exists() {
        return;
    }
    let entry = config.projects.entry(key).or_default();
    entry.has_completed_project_onboarding = true;
    let _ = write_global_config(&config);
}

/// Get the managed settings path (enterprise/MDM).
pub fn managed_settings_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/CoCo/managed-settings.json")
    }
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/etc/coco/managed-settings.json")
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
    {
        PathBuf::from(r"C:\Program Files\CoCo\managed-settings.json")
    }
}

#[cfg(test)]
#[path = "global_config.test.rs"]
mod tests;
