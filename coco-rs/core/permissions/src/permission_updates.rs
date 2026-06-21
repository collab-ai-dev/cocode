//! Permission update application — apply updates to a `ToolPermissionContext`.
//!
//! `applyPermissionUpdate()` / `applyPermissionUpdates()` modify the in-memory
//! context. Persistence to disk is a separate concern (handled by the settings layer).

use coco_types::AdditionalWorkingDir;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRulesBySource;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::ToolPermissionContext;
use tracing::debug;

use crate::rule_compiler;

/// Map an update destination to the corresponding rule source.
fn destination_to_source(dest: PermissionUpdateDestination) -> PermissionRuleSource {
    match dest {
        PermissionUpdateDestination::UserSettings => PermissionRuleSource::UserSettings,
        PermissionUpdateDestination::ProjectSettings => PermissionRuleSource::ProjectSettings,
        PermissionUpdateDestination::LocalSettings => PermissionRuleSource::LocalSettings,
        PermissionUpdateDestination::Session => PermissionRuleSource::Session,
        PermissionUpdateDestination::CliArg => PermissionRuleSource::CliArg,
        PermissionUpdateDestination::Command => PermissionRuleSource::Command,
    }
}

/// Select the rules map (allow/deny/ask) from context based on behavior.
fn rules_map_mut(
    context: &mut ToolPermissionContext,
    behavior: PermissionBehavior,
) -> &mut PermissionRulesBySource {
    match behavior {
        PermissionBehavior::Allow => &mut context.allow_rules,
        PermissionBehavior::Deny => &mut context.deny_rules,
        PermissionBehavior::Ask => &mut context.ask_rules,
    }
}

/// Apply a single permission update to the context and return the modified context.
pub fn apply_permission_update(
    mut context: ToolPermissionContext,
    update: &PermissionUpdate,
) -> ToolPermissionContext {
    match update {
        PermissionUpdate::SetMode { mode } => {
            debug!("applying permission update: setting mode to {mode:?}");
            context.mode = *mode;
        }

        PermissionUpdate::AddRules { rules, destination } => {
            let source = destination_to_source(*destination);
            debug!(
                "applying permission update: adding {} rule(s) to {destination:?}",
                rules.len()
            );

            // Determine behavior from first rule (all rules in one update share behavior)
            if let Some(first) = rules.first() {
                let map = rules_map_mut(&mut context, first.behavior);
                let entry = map.entry(source).or_default();
                entry.extend(rules.iter().cloned());
            }
        }

        PermissionUpdate::ReplaceRules { rules, destination } => {
            let source = destination_to_source(*destination);
            debug!(
                "replacing all rules for {destination:?} with {} rule(s)",
                rules.len()
            );

            if let Some(first) = rules.first() {
                let map = rules_map_mut(&mut context, first.behavior);
                map.insert(source, rules.clone());
            }
        }

        PermissionUpdate::RemoveRules { rules, destination } => {
            let source = destination_to_source(*destination);
            debug!("removing {} rule(s) from {destination:?}", rules.len());

            if let Some(first) = rules.first() {
                let map = rules_map_mut(&mut context, first.behavior);
                if let Some(existing) = map.get_mut(&source) {
                    // Normalize for comparison via roundtrip
                    let to_remove: std::collections::HashSet<String> = rules
                        .iter()
                        .map(|r| rule_compiler::rule_value_to_string(&r.value))
                        .collect();

                    existing.retain(|r| {
                        let normalized = rule_compiler::rule_value_to_string(&r.value);
                        !to_remove.contains(&normalized)
                    });
                }
            }
        }

        PermissionUpdate::AddDirectories {
            directories,
            destination,
        } => {
            debug!(
                "adding {} directories to {destination:?}",
                directories.len()
            );
            for dir in directories {
                context.additional_dirs.insert(
                    dir.clone(),
                    AdditionalWorkingDir {
                        path: dir.clone(),
                        source: *destination,
                    },
                );
            }
        }

        PermissionUpdate::RemoveDirectories { directories, .. } => {
            debug!("removing {} directories", directories.len());
            for dir in directories {
                context.additional_dirs.remove(dir);
            }
        }
    }

    context
}

/// Apply multiple permission updates sequentially.
pub fn apply_permission_updates(
    context: ToolPermissionContext,
    updates: &[PermissionUpdate],
) -> ToolPermissionContext {
    updates.iter().fold(context, apply_permission_update)
}

/// Apply updates directly to the live permission base
/// ([`coco_types::LiveToolPermissionState`] = `ToolAppState.permissions`).
///
/// Builds a transient [`ToolPermissionContext`] view over the live base, folds
/// the updates through the single typed [`apply_permission_updates`] helper, and
/// writes the mutated rule maps / additional dirs / mode back. This is the Rust
/// analog of TS `setToolPermissionContext(applyPermissionUpdate(ctx, update))`
/// — the one mutation path for the main session's shared live rules.
pub fn apply_permission_updates_to_live(
    live: &mut coco_types::LiveToolPermissionState,
    updates: &[PermissionUpdate],
) {
    let ctx = ToolPermissionContext {
        mode: live.mode.unwrap_or(coco_types::PermissionMode::Default),
        additional_dirs: live.additional_dirs.clone(),
        allow_rules: live.allow_rules.clone(),
        deny_rules: live.deny_rules.clone(),
        ask_rules: live.ask_rules.clone(),
        bypass_available: false,
        pre_plan_mode: live.pre_plan_mode,
        stripped_dangerous_rules: live.stripped_dangerous_rules.clone(),
        session_plan_file: None,
        permission_rule_source_roots: live.permission_rule_source_roots.clone(),
    };
    let updated = apply_permission_updates(ctx, updates);
    live.allow_rules = updated.allow_rules;
    live.deny_rules = updated.deny_rules;
    live.ask_rules = updated.ask_rules;
    live.additional_dirs = updated.additional_dirs;
    live.mode = Some(updated.mode);
}

/// Whether a destination supports persistence to disk.
pub fn supports_persistence(dest: PermissionUpdateDestination) -> bool {
    matches!(
        dest,
        PermissionUpdateDestination::LocalSettings
            | PermissionUpdateDestination::UserSettings
            | PermissionUpdateDestination::ProjectSettings
    )
}

/// Extract all rule values from a set of updates.
pub fn extract_rules(updates: &[PermissionUpdate]) -> Vec<&PermissionRule> {
    updates
        .iter()
        .filter_map(|u| match u {
            PermissionUpdate::AddRules { rules, .. } => Some(rules.iter()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Check if any updates contain rules.
pub fn has_rules(updates: &[PermissionUpdate]) -> bool {
    updates
        .iter()
        .any(|u| matches!(u, PermissionUpdate::AddRules { rules, .. } if !rules.is_empty()))
}

#[cfg(test)]
#[path = "permission_updates.test.rs"]
mod tests;
