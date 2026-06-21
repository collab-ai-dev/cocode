//! Builtin plugin registry.
//!
//! Builtin plugins are compiled-in, identified by the marketplace sentinel
//! `{name}@builtin`. They differ from bundled skills in that:
//! - They appear in the `/plugin` UI under a "Built-in" section.
//! - Users can enable/disable them (persisted to settings.json).
//! - They can provide multiple components (skills, hooks, MCP servers, LSP).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

use coco_skills::SkillDefinition;
use serde::Deserialize;
use serde::Serialize;

use crate::identifier::BUILTIN_MARKETPLACE;

/// Acquire the registry lock with poison recovery.
///
/// The registry only holds plain data definitions, so a poisoned mutex
/// (caused by a panic in a previous holder) is safe to recover from —
/// nothing was left in a logically inconsistent state.
fn lock_registry() -> MutexGuard<'static, HashMap<String, BuiltinPluginDefinition>> {
    match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Definition for a builtin plugin shipped in the binary.
#[derive(Clone)]
pub struct BuiltinPluginDefinition {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    /// Default state when the user has not toggled this builtin.
    pub default_enabled: bool,
    /// Optional gate — return `false` to hide entirely (not even as disabled).
    pub is_available: Option<fn() -> bool>,
    /// Skills contributed.
    pub skills: Vec<SkillDefinition>,
    /// Hook config (raw JSON; deserialized by `coco-hooks`).
    pub hooks: Option<serde_json::Value>,
    /// MCP server configs (raw JSON; deserialized by `coco-mcp`).
    pub mcp_servers: HashMap<String, serde_json::Value>,
}

/// Loaded builtin plugin record (after settings resolution).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedBuiltinPlugin {
    pub name: String,
    pub plugin_id: String,
    pub description: String,
    pub version: Option<String>,
    pub enabled: bool,
}

fn registry() -> &'static Mutex<HashMap<String, BuiltinPluginDefinition>> {
    static R: OnceLock<Mutex<HashMap<String, BuiltinPluginDefinition>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a builtin plugin. Call from `init_builtin_plugins()` at startup.
pub fn register_builtin_plugin(def: BuiltinPluginDefinition) {
    lock_registry().insert(def.name.clone(), def);
}

/// Seed the builtin-plugin registry once at startup. This is the single
/// registration point for compiled-in plugins (coco-rs ships no builtins yet).
/// Idempotent: safe to call from every entry point (TUI / SDK / headless). Add
/// `register_builtin_plugin(...)` calls here when a real builtin lands.
pub fn init_builtin_plugins() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        // No builtins registered yet.
    });
}

/// Whether a plugin id has the `@builtin` marketplace suffix.
pub fn is_builtin_plugin_id(id: &str) -> bool {
    id.ends_with(&format!("@{BUILTIN_MARKETPLACE}"))
}

/// Get a builtin definition by name.
pub fn get_builtin_plugin_definition(name: &str) -> Option<BuiltinPluginDefinition> {
    lock_registry().get(name).cloned()
}

/// Get all registered builtin plugins as `LoadedBuiltinPlugin` records,
/// split into enabled/disabled lists.
///
/// `enabled_overrides` is the user's `settings.enabledPlugins` map. State
/// resolution: user override > definition default > true.
pub fn get_builtin_plugins(
    enabled_overrides: &HashMap<String, bool>,
) -> (Vec<LoadedBuiltinPlugin>, Vec<LoadedBuiltinPlugin>) {
    let r = lock_registry();
    let mut enabled = Vec::new();
    let mut disabled = Vec::new();
    for (name, def) in r.iter() {
        if let Some(check) = def.is_available
            && !check()
        {
            continue;
        }
        let plugin_id = format!("{name}@{BUILTIN_MARKETPLACE}");
        let is_enabled = enabled_overrides
            .get(&plugin_id)
            .copied()
            .unwrap_or(def.default_enabled);
        let record = LoadedBuiltinPlugin {
            name: name.clone(),
            plugin_id,
            description: def.description.clone(),
            version: def.version.clone(),
            enabled: is_enabled,
        };
        if is_enabled {
            enabled.push(record);
        } else {
            disabled.push(record);
        }
    }
    (enabled, disabled)
}

/// Get skills contributed by enabled builtin plugins.
pub fn get_builtin_plugin_skills(
    enabled_overrides: &HashMap<String, bool>,
) -> Vec<SkillDefinition> {
    let (enabled, _) = get_builtin_plugins(enabled_overrides);
    let r = lock_registry();
    let mut out = Vec::new();
    for plugin in enabled {
        if let Some(def) = r.get(&plugin.name) {
            out.extend(def.skills.clone());
        }
    }
    out
}

/// Clear the builtin registry — for tests only.
pub fn clear_builtin_plugins() {
    lock_registry().clear();
}

#[cfg(test)]
#[path = "builtins.test.rs"]
mod tests;
