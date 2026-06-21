//! Permission store trait — abstraction for loading/saving permission rules.
//!
//! **Architecture**: This module defines a `PermissionStore` trait that
//! the settings layer (`coco-config`) implements. `coco-permissions` never
//! touches the filesystem directly — it only knows how to evaluate and
//! transform rules. The store trait is the boundary.

use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::ToolPermissionContext;

/// Trait for loading and persisting permission rules.
///
/// Implemented by the settings/config layer to provide disk I/O.
/// `coco-permissions` consumers inject a `&dyn PermissionStore` to
/// load rules at startup and persist user decisions.
pub trait PermissionStore: Send + Sync {
    /// Load all permission rules from all enabled sources.
    ///
    /// Returns rules grouped by source. The implementation reads from
    /// settings files (user, project, local, policy, etc.).
    fn load_all_rules(&self) -> PermissionRulesByBehavior;

    /// Load rules from a specific source.
    fn load_rules_for_source(&self, source: PermissionRuleSource) -> Vec<PermissionRule>;

    /// Persist a permission update to the appropriate settings file.
    ///
    /// Only updates with persistable destinations (userSettings, localSettings,
    /// projectSettings) are written. Session/cliArg updates are ignored.
    fn persist_update(&self, update: &PermissionUpdate) -> Result<(), coco_error::BoxedError>;

    /// Persist multiple updates.
    fn persist_updates(&self, updates: &[PermissionUpdate]) -> Result<(), coco_error::BoxedError> {
        for update in updates {
            self.persist_update(update)?;
        }
        Ok(())
    }

    /// Whether "always allow" options should be shown in permission prompts.
    ///
    /// Returns `false` when managed policy restricts custom rules
    /// (`allowManagedPermissionRulesOnly`).
    fn show_always_allow_options(&self) -> bool {
        true
    }
}

/// Rules grouped by behavior (allow/deny/ask), each containing rules from all sources.
#[derive(Debug, Default)]
pub struct PermissionRulesByBehavior {
    pub allow: Vec<PermissionRule>,
    pub deny: Vec<PermissionRule>,
    pub ask: Vec<PermissionRule>,
}

impl PermissionRulesByBehavior {
    /// Build a `ToolPermissionContext` from loaded rules.
    pub fn into_context(self, mode: coco_types::PermissionMode) -> ToolPermissionContext {
        let mut context = ToolPermissionContext {
            mode,
            additional_dirs: Default::default(),
            permission_rule_source_roots: Default::default(),
            allow_rules: Default::default(),
            deny_rules: Default::default(),
            ask_rules: Default::default(),
            bypass_available: false,
            pre_plan_mode: None,
            stripped_dangerous_rules: None,
            session_plan_file: None,
        };

        for rule in self.allow {
            context
                .allow_rules
                .entry(rule.source)
                .or_default()
                .push(rule);
        }
        for rule in self.deny {
            context
                .deny_rules
                .entry(rule.source)
                .or_default()
                .push(rule);
        }
        for rule in self.ask {
            context.ask_rules.entry(rule.source).or_default().push(rule);
        }

        context
    }
}

/// Whether a destination supports persistence to disk.
///
/// Re-exported from [`crate::permission_updates`] for convenience.
pub fn supports_persistence(dest: PermissionUpdateDestination) -> bool {
    crate::permission_updates::supports_persistence(dest)
}

/// Map behavior string to enum (for parsing settings files).
pub fn parse_behavior(s: &str) -> Option<PermissionBehavior> {
    match s {
        "allow" => Some(PermissionBehavior::Allow),
        "deny" => Some(PermissionBehavior::Deny),
        "ask" => Some(PermissionBehavior::Ask),
        _ => None,
    }
}

#[cfg(test)]
#[path = "permissions_store.test.rs"]
mod tests;
