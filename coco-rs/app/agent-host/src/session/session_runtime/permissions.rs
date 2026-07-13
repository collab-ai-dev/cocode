use std::sync::Arc;

use tokio::sync::RwLock;

use coco_query::QueryEngineConfig;

use super::SessionRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionModeChange {
    pub previous: coco_types::PermissionMode,
    pub changed: bool,
}

/// Build the live permission base (S1) for the main session's `ToolAppState`
/// from the loaded rule maps + mode + dirs + source roots. This is the single
/// seeding shape used at bootstrap, `/clear` re-seed, and the headless/AppServer
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
    pub async fn set_live_permissions(&self, permissions: coco_types::LiveToolPermissionState) {
        self.engine_state_resources
            .app_state()
            .write()
            .await
            .permissions = permissions;
    }

    pub async fn reset_session_permission_rules(&self) -> (usize, usize) {
        let mut guard = self.engine_state_resources.app_state().write().await;
        let cleared_allow_rules = guard
            .permissions
            .allow_rules
            .remove(&coco_types::PermissionRuleSource::Session)
            .map_or(0, |rules| rules.len());
        let cleared_deny_rules = guard
            .permissions
            .deny_rules
            .remove(&coco_types::PermissionRuleSource::Session)
            .map_or(0, |rules| rules.len());
        (cleared_allow_rules, cleared_deny_rules)
    }

    pub async fn set_permission_mode(
        &self,
        mode: coco_types::PermissionMode,
    ) -> PermissionModeChange {
        let fallback_mode = self.current_engine_config().await.permission_mode;
        self.update_engine_config(move |cfg| cfg.permission_mode = mode)
            .await;

        let app_state = self.engine_state_resources.app_state();
        let live_allow_rules = app_state.read().await.permissions.allow_rules.clone();
        let config = self.current_engine_config().await;
        let plan_auto_options = coco_permissions::PlanModeAutoOptions {
            use_auto_mode_during_plan: config.use_auto_mode_during_plan,
            auto_mode_available: config.permission_mode_availability.auto,
        };

        let mut guard = app_state.write().await;
        let previous = guard.permissions.mode.unwrap_or(fallback_mode);
        let changed = coco_permissions::apply_permission_mode_transition_to_app_state(
            &mut guard,
            previous,
            mode,
            &live_allow_rules,
            plan_auto_options,
        );
        PermissionModeChange { previous, changed }
    }

    pub async fn effective_permission_mode(&self) -> coco_types::PermissionMode {
        let live_mode = self
            .engine_state_resources
            .app_state()
            .read()
            .await
            .permissions
            .mode;
        match live_mode {
            Some(mode) => mode,
            None => self.current_engine_config().await.permission_mode,
        }
    }

    pub async fn additional_working_dirs(&self) -> Vec<std::path::PathBuf> {
        self.engine_state_resources
            .app_state()
            .read()
            .await
            .permissions
            .additional_dirs
            .values()
            .map(|dir| std::path::PathBuf::from(&dir.path))
            .collect()
    }

    pub async fn refresh_live_permissions_for_turn(
        &self,
        refresh: super::SessionTurnPermissionRefresh,
    ) {
        let mut guard = self.engine_state_resources.app_state().write().await;
        refresh_live_permissions_for_turn(&mut guard, refresh);
    }

    /// The shared live permission-rule overlay (see field docs). Callers push
    /// mid-cycle approvals / team-rule updates here; every main-session engine
    /// reads it each tool batch.
    pub fn live_permission_rules(&self) -> Arc<RwLock<Vec<coco_types::PermissionRule>>> {
        self.permission_resources.live_permission_rules.clone()
    }

    /// Inject the live permission-rule overlay onto a main-session engine
    /// config. The overlay is now teammate-only: teammate `team_permission_update`
    /// rules never graduate into the live `ToolAppState.permissions` base, so
    /// there is nothing to reconcile/dedup against - the previous base-dedup
    /// step keyed off the now-deleted config rule maps and is gone.
    /// Called from every main-session build path (TUI `build_engine_with_turn_abort`
    /// and AppServer/headless `build_engine_from_config_with_persistence`) so the
    /// in-cycle approval mechanism is uniform across transports. NOT called for
    /// subagents/forks - they keep their own isolated config-cloned rules.
    pub(super) async fn prepare_live_permission_overlay(&self, config: &mut QueryEngineConfig) {
        config.live_permission_rules =
            Some(self.permission_resources.live_permission_rules.clone());
    }

    /// Single source of truth for applying user-approved permission updates,
    /// shared by every transport (TUI dialog, remote approval reply, headless
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
            let mut guard = self.engine_state_resources.app_state().write().await;
            coco_permissions::apply_permission_updates_to_live(&mut guard.permissions, updates);
        }
        // 2. Persist destinations that wire to a settings.json layer.
        let cwd = self.current_cwd().read().await.clone();
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

pub(crate) fn refresh_live_permissions_for_turn(
    guard: &mut coco_types::ToolAppState,
    refresh: super::SessionTurnPermissionRefresh,
) {
    let previous_mode = guard
        .permissions
        .mode
        .unwrap_or(refresh.fallback_previous_mode);
    guard.permissions.allow_rules = refresh.allow_rules;
    guard.permissions.deny_rules = refresh.deny_rules;
    guard.permissions.ask_rules = refresh.ask_rules;
    guard.permissions.permission_rule_source_roots = refresh.permission_rule_source_roots;
    let live_allow_rules = guard.permissions.allow_rules.clone();
    coco_permissions::apply_permission_mode_transition_to_app_state(
        guard,
        previous_mode,
        refresh.permission_mode,
        &live_allow_rules,
        refresh.plan_auto_options,
    );
}
