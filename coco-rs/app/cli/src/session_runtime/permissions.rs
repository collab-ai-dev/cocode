use std::sync::Arc;

use tokio::sync::RwLock;

use coco_query::QueryEngineConfig;

use super::SessionRuntime;

/// Build the live permission base (S1) for the main session's `ToolAppState`
/// from the loaded rule maps + mode + dirs + source roots. This is the single
/// seeding shape used at bootstrap, `/clear` re-seed, and the headless/SDK
/// entry - so the live store (the ONLY permission source the factory
/// reads each batch) always starts from the same source.
pub(crate) fn live_permissions(
    mode: coco_types::PermissionMode,
    allow_rules: coco_types::PermissionRulesBySource,
    deny_rules: coco_types::PermissionRulesBySource,
    ask_rules: coco_types::PermissionRulesBySource,
    additional_dirs: std::collections::HashMap<String, coco_types::AdditionalWorkingDir>,
    permission_rule_source_roots: std::collections::HashMap<
        coco_types::PermissionRuleSource,
        std::path::PathBuf,
    >,
) -> coco_types::LiveToolPermissionState {
    coco_types::LiveToolPermissionState {
        mode: Some(mode),
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        allow_rules,
        deny_rules,
        ask_rules,
        additional_dirs,
        permission_rule_source_roots,
    }
}

impl SessionRuntime {
    /// The shared live permission-rule overlay (see field docs). Callers push
    /// mid-cycle approvals / team-rule updates here; every main-session engine
    /// reads it each tool batch.
    pub fn live_permission_rules(&self) -> Arc<RwLock<Vec<coco_types::PermissionRule>>> {
        self.live_permission_rules.clone()
    }

    /// Inject the live permission-rule overlay onto a main-session engine
    /// config. The overlay is now teammate-only: teammate `team_permission_update`
    /// rules never graduate into the live `ToolAppState.permissions` base, so
    /// there is nothing to reconcile/dedup against - the previous base-dedup
    /// step keyed off the now-deleted config rule maps and is gone.
    /// Called from every main-session build path (TUI `build_engine_with_turn_abort`
    /// and SDK/headless `build_engine_from_config_with_persistence`) so the
    /// in-cycle approval mechanism is uniform across transports. NOT called for
    /// subagents/forks - they keep their own isolated config-cloned rules.
    pub(super) async fn prepare_live_permission_overlay(&self, config: &mut QueryEngineConfig) {
        config.live_permission_rules = Some(self.live_permission_rules.clone());
    }

    /// Single source of truth for applying user-approved permission updates,
    /// shared by every transport (TUI dialog, SDK approval reply, headless
    /// permission-prompt tool). Lands the rules in both places they must
    /// reach so in-cycle and cross-cycle behavior stays aligned:
    /// 1. the live `ToolAppState.permissions` base - the single authoritative
    /// source the factory reads each batch (shared by Arc with the in-flight
    /// engine + subagents/forks), so the approval is visible THIS cycle on
    /// the next batch (e.g. an `Edit(...)` grant then satisfies a same-cycle
    /// Read via the "edit access implies read" branch) AND across cycles;
    /// 2. disk, for destinations that persist (User/Project/Local).
    /// This is coco-rs's analog of `applyPermissionUpdate` +
    /// `setToolPermissionContext`.
    pub async fn apply_permission_updates_everywhere(
        &self,
        updates: &[coco_types::PermissionUpdate],
    ) {
        if updates.is_empty() {
            return;
        }
        // 1. Mutate the live shared base (`ToolAppState.permissions`) - the
        // single authoritative source the factory reads each batch, shared by
        // Arc with subagents/forks. This is both the in-cycle (the in-flight
        // engine re-reads it next batch) and cross-cycle home for the rule.
        {
            let mut guard = self.app_state.write().await;
            coco_permissions::apply_permission_updates_to_live(&mut guard.permissions, updates);
        }
        // 2. Persist destinations that wire to a settings.json layer.
        let cwd = self.current_cwd.read().await.clone();
        let store = coco_permissions::SettingsPermissionStore::new(cwd);
        use coco_permissions::permissions_store::PermissionStore;
        for update in updates {
            let Some(dest) = update.destination() else {
                continue;
            };
            if !coco_permissions::permission_updates::supports_persistence(dest) {
                continue;
            }
            if let Err(e) = store.persist_update(update) {
                tracing::warn!(error = %e, "failed to persist permission update");
            }
        }
        let applied: Vec<String> = updates
            .iter()
            .filter_map(|u| match u {
                coco_types::PermissionUpdate::AddRules { rules, destination } => Some(
                    rules
                        .iter()
                        .map(|r| {
                            format!(
                                "{destination:?}:{}({})",
                                r.value.tool_pattern,
                                r.value.rule_content.as_deref().unwrap_or("*")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                _ => None,
            })
            .collect();
        tracing::info!(
            applied = ?applied,
            "permission updates applied to live base (ToolAppState.permissions) + disk persist",
        );
    }
}
