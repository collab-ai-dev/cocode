use std::sync::Arc;

use anyhow::Result;

use crate::project_services::ProjectServices;
use crate::project_services::project_registry;

use super::SessionRuntime;

impl SessionRuntime {
    /// Re-register plugin-contributed MCP servers with the attached
    /// `McpConnectionManager` and bump the reconnect key. Called from
    /// `/reload-plugins` (and after install / delisting) so a plugin
    /// enable/disable flows into the MCP layer.
    /// Reconciles the live MCP set against the currently-enabled plugins:
    /// servers from now-disabled/uninstalled plugins (the `plugin:` namespace)
    /// are unregistered + their tools deregistered; newly-enabled servers are
    /// registered, connected, and their tools registered. Bumps the reconnect
    /// key. No-op (returns 0) when no manager is attached. Returns the count of
    /// currently-enabled plugin MCP servers.
    pub async fn reload_plugin_mcp_servers(&self) -> usize {
        let project_services =
            project_registry().reload(&self.config_home, self.project_root.clone());
        self.reload_plugin_mcp_servers_with(project_services).await
    }

    async fn reload_plugin_mcp_servers_with(
        &self,
        project_services: Arc<ProjectServices>,
    ) -> usize {
        let Some(manager) = self.mcp_manager.read().await.clone() else {
            return 0;
        };
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> =
            project_services.plugins().iter().collect();
        let scoped = coco_plugins::mcp_bridge::extract_mcp_servers_from_plugins(&plugin_refs);
        let count = scoped.len();
        let new_names: std::collections::HashSet<String> =
            scoped.iter().map(|s| s.name.clone()).collect();

        // Reconcile: drop plugin servers (`plugin:` namespace) no longer present,
        // then (re)register the current set. Plugin servers are keyed
        // `plugin:<plugin>:<server>` by `mcp_bridge`, so the prefix isolates them
        // from config-file servers, which this reload must never touch.
        let stale: Vec<String> = {
            let mut mgr = manager.lock().await;
            let stale: Vec<String> = mgr
                .registered_server_names()
                .into_iter()
                .filter(|n| n.starts_with("plugin:") && !new_names.contains(n))
                .collect();
            for name in &stale {
                mgr.unregister_server(name).await;
            }
            mgr.register_all(scoped);
            stale
        };
        for name in &stale {
            coco_tools::deregister_mcp_server(self.tools(), name);
        }
        if count > 0 {
            // Connect the newly-registered servers + register their tools into the
            // live registry (idempotent — already-connected servers are skipped).
            crate::session_bootstrap::connect_and_register_mcp(
                manager.clone(),
                self.tools().clone(),
            )
            .await;
        }
        self.mcp_reconnect_key
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        count
    }

    /// Re-read the on-disk LSP config, re-merge plugin-contributed LSP servers,
    /// and re-prewarm via the attached LSP handle. No-op when no handle is
    /// attached. Called from `/reload-plugins`.
    pub async fn reload_lsp_servers(&self) {
        if let Some(handle) = self.current_lsp_handle().await {
            handle.reload(&self.project_root).await;
        }
    }

    /// Snapshot the current command registry. Cheap (single Arc clone).
    /// Callers should hold the snapshot for the duration of one
    /// dispatch — a concurrent `/reload-plugins` may swap the inner
    /// Arc, but existing snapshots stay live until dropped.
    pub async fn current_command_registry(&self) -> Arc<coco_commands::CommandRegistry> {
        self.command_registry.read().await.clone()
    }

    /// Rebuild the slash-command registry from disk and atomically
    /// swap it in. Triggered by `/reload-plugins` so the user can pick
    /// up plugin / skill / command edits without restarting the
    /// session. A new `SkillManager` and a freshly resolved enabled plugin
    /// set (`load_enabled_plugins`) are constructed each call; resolution
    /// order matches the original bootstrap
    /// (`commands::build_command_registry`).
    /// Uses the frozen [`Self::runtime_config`] snapshot — fine for
    /// the user-initiated `/reload-plugins` path where settings
    /// haven't been mutated. Callers that just wrote to
    /// `settings.local.json` must use [`Self::reload_plugins_with`]
    /// to pass the freshly-republished `RuntimeConfig` (otherwise
    /// the registry rebuild reads stale `skill_overrides` tiers).
    /// Returns the count of registered commands in the new registry
    /// so the caller can show the user a confirmation.
    pub async fn reload_plugins(&self, cwd: &std::path::Path) -> usize {
        self.reload_plugins_with(cwd, &self.runtime_config).await
    }

    /// Variant of [`Self::reload_plugins`] that takes an explicit
    /// `RuntimeConfig`. Use this when the caller has just mutated
    /// settings (e.g. `/skills` dialog save) and the publisher's
    /// `current()` snapshot is fresher than [`Self::runtime_config`].
    pub async fn reload_plugins_with(
        &self,
        cwd: &std::path::Path,
        runtime_config: &coco_config::RuntimeConfig,
    ) -> usize {
        // Reload the LIVE skill manager (the one feeding the model catalog +
        // dispatch), not a throwaway. Build a fresh catalog off disk with the
        // resolved load gates, fold it into `self.skill_manager`, and clear the
        // announcement map so edited skills re-announce.
        let gates = crate::session_bootstrap::resolve_skill_load_gates_with_add_dirs(
            runtime_config,
            cwd,
            &[],
        );
        let fresh = coco_skills::build_session_skill_manager(&self.config_home, cwd, &gates);
        let fresh_skills: Vec<_> = fresh
            .all_including_conditional()
            .into_iter()
            .map(|skill| (*skill).clone())
            .collect();
        self.skill_manager.reload_disk_skills(fresh_skills);
        self.skill_manager.reset_announcements();
        // Unified V2 plugin load (marketplace cache + project inline dirs,
        // gated by enabled_plugins) so a `/reload-plugins` picks up
        // disable/enable and marketplace installs, registering real
        // plugin-command bodies.
        let project_services =
            project_registry().reload(&self.config_home, self.project_root.clone());
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> =
            project_services.plugins().iter().collect();
        for skill in coco_plugins::skill_bridge::load_all_plugin_skills_v2(&plugin_refs) {
            self.skill_manager.register(skill);
        }
        // Builtin plugin skills — symmetric with bootstrap so a reload
        // re-registers them too.
        coco_plugins::builtins::init_builtin_plugins();
        for skill in coco_plugins::builtin_plugin_skills(&self.config_home) {
            self.skill_manager.register(skill);
        }
        let mut command_features = runtime_config.features.clone();
        if !runtime_config.memory_activation.active {
            command_features.disable(coco_types::Feature::AutoMemory);
        }
        let registry = coco_commands::build_command_registry(
            &self.skill_manager,
            project_services.plugins(),
            coco_types::UserType::from_env(),
            command_features,
            runtime_config.loop_config.clone(),
            cwd.to_path_buf(),
            dirs::home_dir().unwrap_or_else(|| cwd.to_path_buf()),
            None,
            &runtime_config.skill_overrides,
        );
        registry.set_build_provenance(crate::build_provenance());
        let count = registry.len();
        let new_registry = Arc::new(registry);
        {
            let mut slot = self.command_registry.write().await;
            *slot = new_registry;
        }
        // Re-register plugin MCP servers with the live manager (if attached) so a
        // reload picks up newly enabled/disabled plugin MCP contributions.
        // No-op without a manager (e.g. the TUI today).
        let _ = self
            .reload_plugin_mcp_servers_with(project_services.clone())
            .await;
        count
    }

    /// Reload the live `HookRegistry` from the latest `RuntimeConfig`
    /// snapshot (settings + plugin hooks). Triggered by `/hooks reload`.
    /// Atomic semantics:
    /// - Settings hooks (User/Project/Local/Flag/Policy scopes) and
    /// plugin hooks are both rebuilt.
    /// - `fired_once` set is **preserved** so a `once` hook that
    /// already fired this session doesn't re-fire after reload.
    /// - Per-agent `agent_scoped` hook layer is **preserved** — those are
    /// owned by the coordinator's spawn lifecycle, not by settings.
    /// - Slash commands run only at turn boundaries (the dispatch loop
    /// in `tui_runner` `drain_active_turn`s before invoking them),
    /// so PreToolUse/PostToolUse for an in-flight call cannot see
    /// different hook sets.
    /// Returns the count of hooks now registered.
    pub async fn reload_hooks(&self) -> Result<usize> {
        let policy = coco_hooks::LoaderPolicy {
            disable_all_hooks: self.runtime_config.settings.merged.disable_all_hooks,
            allow_managed_hooks_only: self.runtime_config.settings.merged.allow_managed_hooks_only,
        };

        // Build (scope, value) pairs for every active settings source.
        // Plugin hooks are layered separately because they live on
        // disk inside plugin directories, not in settings.json.
        let mut sources: Vec<(coco_types::HookScope, serde_json::Value)> = Vec::new();
        for source in [
            coco_config::SettingSource::User,
            coco_config::SettingSource::Project,
            coco_config::SettingSource::Local,
            coco_config::SettingSource::Flag,
            coco_config::SettingSource::Policy,
        ] {
            let Some(value) = self.runtime_config.settings.per_source.get(&source) else {
                continue;
            };
            let Some(hooks_value) = value.get("hooks") else {
                continue;
            };
            let scope = match source {
                coco_config::SettingSource::User => coco_types::HookScope::User,
                coco_config::SettingSource::Project => coco_types::HookScope::Project,
                coco_config::SettingSource::Local => coco_types::HookScope::Local,
                coco_config::SettingSource::Flag => coco_types::HookScope::Local,
                coco_config::SettingSource::Policy => coco_types::HookScope::Policy,
                coco_config::SettingSource::Plugin => coco_types::HookScope::Plugin,
            };
            sources.push((scope, hooks_value.clone()));
        }

        // Atomic settings-hook swap.
        let settings_count = self
            .hook_registry
            .reload_from_runtime(&sources, policy)
            .map_err(|e| anyhow::anyhow!("hook reload failed: {e}"))?;

        // Re-layer plugin hooks on top — they aren't in settings.json
        // so `reload_from_runtime` doesn't see them. Unified V2 source.
        let project_services =
            project_registry().reload(&self.config_home, self.project_root.clone());
        if !project_services.plugins().is_empty() {
            let refs: Vec<&coco_plugins::loader::LoadedPluginV2> =
                project_services.plugins().iter().collect();
            coco_plugins::hook_bridge::register_plugin_hooks_v2(&self.hook_registry, &refs);
        }

        Ok(self.hook_registry.len().max(settings_count))
    }
}
