//! Concrete `PermissionStore` implementation backed by settings files on disk.
//!
//! Reads permission rules from settings files (user, project, local, policy)
//! and persists "always allow" / "always deny" updates back to disk.

use std::path::Path;
use std::path::PathBuf;

use coco_config::global_config;
use coco_config::settings::PermissionsConfig;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use tracing::debug;
use tracing::warn;

use crate::permissions_store::PermissionRulesByBehavior;
use crate::permissions_store::PermissionStore;
use crate::rule_compiler;

/// `PermissionStore` backed by JSON settings files on disk.
///
/// Reads rules from:
/// - `~/.coco/settings.json` (userSettings)
/// - `.coco/settings.json` (projectSettings)
/// - `.coco/settings.local.json` (localSettings)
/// - Managed settings path (policySettings)
///
/// Writes rules back to the corresponding file when persisting updates.
pub struct SettingsPermissionStore {
    cwd: PathBuf,
    /// Optional path from `--settings` CLI flag.
    flag_settings_path: Option<PathBuf>,
}

impl SettingsPermissionStore {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            flag_settings_path: None,
        }
    }

    /// Create a store with a flag settings path from `--settings` CLI flag.
    pub fn with_flag_settings(mut self, path: impl Into<PathBuf>) -> Self {
        self.flag_settings_path = Some(path.into());
        self
    }

    /// Load the permissions config from a single settings file.
    fn load_permissions_from_file(path: &Path) -> Option<PermissionsConfig> {
        let contents = std::fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
        let perms = value.get("permissions")?;
        serde_json::from_value(perms.clone()).ok()
    }

    /// Convert rule strings from a `PermissionsConfig` into typed `PermissionRule`s.
    fn config_to_rules(
        config: &PermissionsConfig,
        source: PermissionRuleSource,
    ) -> Vec<PermissionRule> {
        let mut rules = Vec::new();
        for rule_str in &config.allow {
            let value = rule_compiler::parse_rule_string(rule_str);
            rules.push(PermissionRule {
                source,
                behavior: PermissionBehavior::Allow,
                value,
            });
        }
        for rule_str in &config.deny {
            let value = rule_compiler::parse_rule_string(rule_str);
            rules.push(PermissionRule {
                source,
                behavior: PermissionBehavior::Deny,
                value,
            });
        }
        for rule_str in &config.ask {
            let value = rule_compiler::parse_rule_string(rule_str);
            rules.push(PermissionRule {
                source,
                behavior: PermissionBehavior::Ask,
                value,
            });
        }
        rules
    }

    /// All sources with their file paths.
    ///
    /// Order: user → project → local → flag → policy. Later sources override earlier ones.
    fn source_paths(&self) -> Vec<(PermissionRuleSource, PathBuf)> {
        let mut sources = vec![
            (
                PermissionRuleSource::UserSettings,
                global_config::user_settings_path(),
            ),
            (
                PermissionRuleSource::ProjectSettings,
                global_config::project_settings_path(&self.cwd),
            ),
            (
                PermissionRuleSource::LocalSettings,
                global_config::local_settings_path(&self.cwd),
            ),
        ];

        // Flag settings from --settings CLI flag
        if let Some(flag_path) = &self.flag_settings_path {
            sources.push((PermissionRuleSource::FlagSettings, flag_path.clone()));
        }

        sources.push((
            PermissionRuleSource::PolicySettings,
            global_config::managed_settings_path(),
        ));

        sources
    }

    /// Resolve file path for a destination.
    fn path_for_destination(&self, dest: PermissionUpdateDestination) -> Option<PathBuf> {
        match dest {
            PermissionUpdateDestination::UserSettings => Some(global_config::user_settings_path()),
            PermissionUpdateDestination::ProjectSettings => {
                Some(global_config::project_settings_path(&self.cwd))
            }
            PermissionUpdateDestination::LocalSettings => {
                Some(global_config::local_settings_path(&self.cwd))
            }
            PermissionUpdateDestination::Session
            | PermissionUpdateDestination::CliArg
            | PermissionUpdateDestination::Command => None,
        }
    }

    /// Read a settings file as raw JSON, preserving all fields.
    fn read_settings_json(path: &Path) -> serde_json::Value {
        match std::fs::read_to_string(path) {
            Ok(contents) if !contents.trim().is_empty() => {
                serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
            }
            _ => serde_json::json!({}),
        }
    }

    /// Write a settings JSON value to disk, creating directories as needed.
    fn write_settings_json(
        path: &Path,
        value: &serde_json::Value,
    ) -> Result<(), coco_error::BoxedError> {
        use coco_error::StatusCode;
        use coco_error::boxed;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| boxed(e, StatusCode::IoError))?;
        }
        let contents =
            serde_json::to_string_pretty(value).map_err(|e| boxed(e, StatusCode::Internal))?;
        std::fs::write(path, contents).map_err(|e| boxed(e, StatusCode::IoError))?;
        Ok(())
    }

    /// Load every file-backed additional working directory, tagged with
    /// the source layer that contributed it. Drives the `/permissions`
    /// editor's Workspace tab. Sources are read in the same priority order
    /// as [`Self::source_paths`]; duplicates across layers are preserved so
    /// the editor can show (and remove from) each layer independently.
    pub fn load_additional_directories(&self) -> Vec<(PermissionRuleSource, String)> {
        let mut dirs = Vec::new();
        if self.is_managed_only() {
            return dirs;
        }
        for (source, path) in self.source_paths() {
            if let Some(config) = Self::load_permissions_from_file(&path) {
                for dir in config.additional_directories {
                    dirs.push((source, dir));
                }
            }
        }
        dirs
    }

    /// Check if managed policy restricts to only managed rules.
    fn is_managed_only(&self) -> bool {
        let policy_path = global_config::managed_settings_path();
        if let Some(config) = Self::load_permissions_from_file(&policy_path) {
            return config.allow_managed_permission_rules_only;
        }
        false
    }

    /// Persist added rules to a settings file.
    fn persist_add_rules(
        &self,
        rules: &[PermissionRule],
        dest: PermissionUpdateDestination,
    ) -> Result<(), coco_error::BoxedError> {
        if rules.is_empty() {
            return Ok(());
        }
        let behavior_key = match rules[0].behavior {
            PermissionBehavior::Allow => "allow",
            PermissionBehavior::Deny => "deny",
            PermissionBehavior::Ask => "ask",
        };
        let path = match self.path_for_destination(dest) {
            Some(p) => p,
            None => return Ok(()),
        };
        if self.is_managed_only() {
            warn!("blocked: cannot add rules — allowManagedPermissionRulesOnly is enabled");
            return Ok(());
        }

        let mut settings = Self::read_settings_json(&path);
        // Ensure the top level is an object; replace otherwise.
        if !settings.is_object() {
            settings = serde_json::json!({});
        }
        let Some(settings_obj) = settings.as_object_mut() else {
            return Ok(());
        };
        let permissions = settings_obj
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}));
        if !permissions.is_object() {
            *permissions = serde_json::json!({});
        }
        let Some(permissions_obj) = permissions.as_object_mut() else {
            return Ok(());
        };
        let arr = permissions_obj
            .entry(behavior_key)
            .or_insert_with(|| serde_json::json!([]));
        if !arr.is_array() {
            *arr = serde_json::json!([]);
        }
        let Some(existing) = arr.as_array_mut() else {
            return Ok(());
        };

        // Normalize existing entries via roundtrip to prevent duplicates
        let existing_normalized: std::collections::HashSet<String> = existing
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| {
                let parsed = rule_compiler::parse_rule_string(s);
                rule_compiler::rule_value_to_string(&parsed)
            })
            .collect();

        for rule in rules {
            let rule_str = rule_compiler::rule_value_to_string(&rule.value);
            if !existing_normalized.contains(&rule_str) {
                existing.push(serde_json::Value::String(rule_str));
            }
        }

        Self::write_settings_json(&path, &settings)?;
        debug!("persisted {behavior_key} rules to {}", path.display());
        Ok(())
    }

    /// Replace the entire `[behavior]` rule array on a settings file.
    ///
    /// Unlike `persist_add_rules`, this overwrites the array wholesale —
    /// used by the rules editor UI when a user reorders or wholesale-edits
    /// permissions for a single source.
    ///
    /// Empty `rules` is a no-op: `PermissionUpdate::ReplaceRules` derives
    /// behavior from the first rule. With no rules there's nothing to anchor
    /// the target key, so we can't faithfully clear a specific behavior list.
    /// Matches `permission_updates.rs:78-81`'s in-memory variant of the same operation.
    fn persist_replace_rules(
        &self,
        rules: &[PermissionRule],
        dest: PermissionUpdateDestination,
    ) -> Result<(), coco_error::BoxedError> {
        let Some(first) = rules.first() else {
            return Ok(());
        };
        let path = match self.path_for_destination(dest) {
            Some(p) => p,
            None => return Ok(()),
        };
        if self.is_managed_only() {
            warn!("blocked: cannot replace rules — allowManagedPermissionRulesOnly is enabled");
            return Ok(());
        }
        let behavior_key = match first.behavior {
            PermissionBehavior::Allow => "allow",
            PermissionBehavior::Deny => "deny",
            PermissionBehavior::Ask => "ask",
        };

        let mut settings = Self::read_settings_json(&path);
        if !settings.is_object() {
            settings = serde_json::json!({});
        }
        let Some(settings_obj) = settings.as_object_mut() else {
            return Ok(());
        };
        let permissions = settings_obj
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}));
        if !permissions.is_object() {
            *permissions = serde_json::json!({});
        }
        let Some(permissions_obj) = permissions.as_object_mut() else {
            return Ok(());
        };
        let rule_strings: Vec<serde_json::Value> = rules
            .iter()
            .map(|r| serde_json::Value::String(rule_compiler::rule_value_to_string(&r.value)))
            .collect();
        permissions_obj.insert(
            behavior_key.to_string(),
            serde_json::Value::Array(rule_strings),
        );

        Self::write_settings_json(&path, &settings)?;
        debug!("replaced {behavior_key} rules in {}", path.display());
        Ok(())
    }

    /// Persist `additionalDirectories` additions to a settings file.
    ///
    /// Reads the existing list, appends new dirs while preserving order and
    /// dropping duplicates, then rewrites the file.
    fn persist_add_directories(
        &self,
        directories: &[String],
        dest: PermissionUpdateDestination,
    ) -> Result<(), coco_error::BoxedError> {
        if directories.is_empty() {
            return Ok(());
        }
        let path = match self.path_for_destination(dest) {
            Some(p) => p,
            None => return Ok(()),
        };
        if self.is_managed_only() {
            warn!(
                "blocked: cannot add additionalDirectories — allowManagedPermissionRulesOnly is enabled"
            );
            return Ok(());
        }

        let mut settings = Self::read_settings_json(&path);
        if !settings.is_object() {
            settings = serde_json::json!({});
        }
        let Some(settings_obj) = settings.as_object_mut() else {
            return Ok(());
        };
        let permissions = settings_obj
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}));
        if !permissions.is_object() {
            *permissions = serde_json::json!({});
        }
        let Some(permissions_obj) = permissions.as_object_mut() else {
            return Ok(());
        };
        let arr = permissions_obj
            .entry("additionalDirectories")
            .or_insert_with(|| serde_json::json!([]));
        if !arr.is_array() {
            *arr = serde_json::json!([]);
        }
        let Some(existing) = arr.as_array_mut() else {
            return Ok(());
        };

        let existing_set: std::collections::HashSet<String> = existing
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();

        for dir in directories {
            if !existing_set.contains(dir) {
                existing.push(serde_json::Value::String(dir.clone()));
            }
        }

        Self::write_settings_json(&path, &settings)?;
        debug!("persisted additionalDirectories to {}", path.display());
        Ok(())
    }

    /// Persist `additionalDirectories` removals to a settings file.
    fn persist_remove_directories(
        &self,
        directories: &[String],
        dest: PermissionUpdateDestination,
    ) -> Result<(), coco_error::BoxedError> {
        if directories.is_empty() {
            return Ok(());
        }
        let path = match self.path_for_destination(dest) {
            Some(p) => p,
            None => return Ok(()),
        };
        if self.is_managed_only() {
            warn!(
                "blocked: cannot remove additionalDirectories — allowManagedPermissionRulesOnly is enabled"
            );
            return Ok(());
        }

        let mut settings = Self::read_settings_json(&path);
        let permissions = match settings.get_mut("permissions") {
            Some(p) => p,
            None => return Ok(()),
        };
        let arr = match permissions.get_mut("additionalDirectories") {
            Some(a) => a,
            None => return Ok(()),
        };
        let existing = match arr.as_array_mut() {
            Some(a) => a,
            None => return Ok(()),
        };

        let to_remove: std::collections::HashSet<&str> =
            directories.iter().map(String::as_str).collect();
        existing.retain(|v| match v.as_str() {
            Some(s) => !to_remove.contains(s),
            None => true,
        });

        Self::write_settings_json(&path, &settings)?;
        debug!("removed additionalDirectories from {}", path.display());
        Ok(())
    }

    /// Persist rule removal to a settings file.
    ///
    /// Normalizes entries via roundtrip parse→serialize so legacy names match.
    fn persist_remove_rules(
        &self,
        rules: &[PermissionRule],
        dest: PermissionUpdateDestination,
    ) -> Result<(), coco_error::BoxedError> {
        if rules.is_empty() {
            return Ok(());
        }
        let behavior_key = match rules[0].behavior {
            PermissionBehavior::Allow => "allow",
            PermissionBehavior::Deny => "deny",
            PermissionBehavior::Ask => "ask",
        };
        let path = match self.path_for_destination(dest) {
            Some(p) => p,
            None => return Ok(()),
        };
        if self.is_managed_only() {
            warn!("blocked: cannot remove rules — allowManagedPermissionRulesOnly is enabled");
            return Ok(());
        }

        let mut settings = Self::read_settings_json(&path);
        let permissions = match settings.get_mut("permissions") {
            Some(p) => p,
            None => return Ok(()),
        };
        let arr = match permissions.get_mut(behavior_key) {
            Some(a) => a,
            None => return Ok(()),
        };
        let existing = match arr.as_array_mut() {
            Some(a) => a,
            None => return Ok(()),
        };

        // Build set of normalized rule strings to remove
        let to_remove: std::collections::HashSet<String> = rules
            .iter()
            .map(|r| rule_compiler::rule_value_to_string(&r.value))
            .collect();

        // Filter out matching rules (normalize via roundtrip for legacy names)
        existing.retain(|v| {
            let Some(raw) = v.as_str() else { return true };
            let parsed = rule_compiler::parse_rule_string(raw);
            let normalized = rule_compiler::rule_value_to_string(&parsed);
            !to_remove.contains(&normalized)
        });

        Self::write_settings_json(&path, &settings)?;
        debug!("removed {behavior_key} rules from {}", path.display());
        Ok(())
    }
}

impl PermissionStore for SettingsPermissionStore {
    fn load_all_rules(&self) -> PermissionRulesByBehavior {
        let mut result = PermissionRulesByBehavior::default();

        // If managed-only, only load policy rules
        if self.is_managed_only() {
            let policy_path = global_config::managed_settings_path();
            if let Some(config) = Self::load_permissions_from_file(&policy_path) {
                let rules = Self::config_to_rules(&config, PermissionRuleSource::PolicySettings);
                for rule in rules {
                    match rule.behavior {
                        PermissionBehavior::Allow => result.allow.push(rule),
                        PermissionBehavior::Deny => result.deny.push(rule),
                        PermissionBehavior::Ask => result.ask.push(rule),
                    }
                }
            }
            return result;
        }

        for (source, path) in self.source_paths() {
            if let Some(config) = Self::load_permissions_from_file(&path) {
                let rules = Self::config_to_rules(&config, source);
                for rule in rules {
                    match rule.behavior {
                        PermissionBehavior::Allow => result.allow.push(rule),
                        PermissionBehavior::Deny => result.deny.push(rule),
                        PermissionBehavior::Ask => result.ask.push(rule),
                    }
                }
            }
        }

        result
    }

    fn load_rules_for_source(&self, source: PermissionRuleSource) -> Vec<PermissionRule> {
        let path = match source {
            PermissionRuleSource::UserSettings => global_config::user_settings_path(),
            PermissionRuleSource::ProjectSettings => {
                global_config::project_settings_path(&self.cwd)
            }
            PermissionRuleSource::LocalSettings => global_config::local_settings_path(&self.cwd),
            PermissionRuleSource::PolicySettings => global_config::managed_settings_path(),
            // CLI, session, command, and flag sources are not file-backed
            _ => return vec![],
        };

        Self::load_permissions_from_file(&path)
            .map(|config| Self::config_to_rules(&config, source))
            .unwrap_or_default()
    }

    fn persist_update(&self, update: &PermissionUpdate) -> Result<(), coco_error::BoxedError> {
        match update {
            PermissionUpdate::AddRules { rules, destination } => {
                self.persist_add_rules(rules, *destination)
            }
            PermissionUpdate::ReplaceRules { rules, destination } => {
                self.persist_replace_rules(rules, *destination)
            }
            PermissionUpdate::RemoveRules { rules, destination } => {
                self.persist_remove_rules(rules, *destination)
            }
            PermissionUpdate::AddDirectories {
                directories,
                destination,
            } => self.persist_add_directories(directories, *destination),
            PermissionUpdate::RemoveDirectories {
                directories,
                destination,
            } => self.persist_remove_directories(directories, *destination),
            // SetMode has no destination — it changes session state,
            // not a settings layer.
            PermissionUpdate::SetMode { .. } => Ok(()),
        }
    }

    fn show_always_allow_options(&self) -> bool {
        !self.is_managed_only()
    }
}

#[cfg(test)]
#[path = "settings_store.test.rs"]
mod tests;
