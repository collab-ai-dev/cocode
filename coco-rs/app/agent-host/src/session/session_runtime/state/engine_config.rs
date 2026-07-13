use super::*;

impl SessionRuntime {
    pub async fn set_agent_progress_summaries_enabled(&self, enabled: bool) {
        self.engine_state_resources
            .app_state()
            .write()
            .await
            .agent_progress_summaries_enabled = enabled;
    }
    pub fn model_runtimes(&self) -> Arc<coco_inference::ModelRuntimeRegistry> {
        self.execution.model_runtimes()
    }
    /// Resolve a client/user-supplied model string into a concrete
    /// provider/model pair using the same registry snapshot that built
    /// this session. `provider/model_id` is accepted directly; bare
    /// model ids first bind to the current Main provider, then to the
    /// deterministic provider catalog order.
    pub fn resolve_model_selection(&self, raw_model: &str) -> Option<ProviderModelSelection> {
        resolve_model_selection_from_runtime_config(self.runtime_config(), raw_model)
    }
    /// Snapshot the current `QueryEngineConfig` (clones the inner struct).
    /// Per-turn engine builds use this so mid-session mutations like
    /// `set_permission_mode` propagate immediately.
    pub async fn current_engine_config(&self) -> QueryEngineConfig {
        self.engine_config_resources
            .engine_config()
            .read()
            .await
            .clone()
    }
    /// Mutate `engine_config` under lock. Use for mid-session updates
    /// like `SetPermissionMode`.
    pub async fn update_engine_config<F>(&self, f: F)
    where
        F: FnOnce(&mut QueryEngineConfig),
    {
        let snapshot = {
            let mut g = self.engine_config_resources.engine_config().write().await;
            // Session identity no longer lives on the mutable engine config
            // (it is owned immutably by `SessionEngineConfigResources`), so a
            // config edit structurally cannot rotate it.
            f(&mut g);
            g.clone()
        };
        write_std_rwlock(
            self.engine_config_resources.orchestration_engine_config(),
            snapshot,
        );
    }
    pub async fn set_model_id(&self, model_id: String) -> String {
        let old_model = self.current_engine_config().await.model_id;
        self.update_engine_config(move |engine_config| {
            engine_config.model_id = model_id;
        })
        .await;
        old_model
    }
    pub async fn set_thinking_level(&self, thinking_level: Option<coco_types::ThinkingLevel>) {
        self.update_engine_config(move |engine_config| {
            engine_config.thinking_level = thinking_level;
        })
        .await;
    }
    pub async fn set_fast_mode(&self, active: bool) {
        self.update_engine_config(move |engine_config| {
            engine_config.fast_mode = active;
        })
        .await;
    }
    pub async fn set_requires_structured_output(&self, active: bool) {
        self.update_engine_config(move |engine_config| {
            engine_config.requires_structured_output = active;
        })
        .await;
    }
    pub async fn set_skill_overrides(&self, skill_overrides: Arc<coco_config::SkillOverrideTiers>) {
        self.update_engine_config(move |engine_config| {
            engine_config.skill_overrides = skill_overrides;
        })
        .await;
    }
    pub async fn apply_session_start_config(&self, config: super::SessionStartRuntimeConfig) {
        let model_id = config.model_id;
        let permission_mode = config.permission_mode;
        let max_turns = config.max_turns;
        let max_budget_usd = config.max_budget_usd;
        let system_prompt = config.system_prompt;
        let append_system_prompt = config.append_system_prompt;
        let plan_mode_custom_instructions = config.plan_mode_custom_instructions;
        let requires_structured_output = config.requires_structured_output;
        self.update_engine_config(move |engine_config| {
            if let Some(model_id) = model_id {
                engine_config.model_id = model_id;
            }
            if let Some(permission_mode) = permission_mode {
                engine_config.permission_mode = permission_mode;
            }
            if max_turns.is_some() {
                engine_config.max_turns = max_turns;
            }
            if max_budget_usd.is_some() {
                engine_config.max_budget_usd = max_budget_usd;
            }
            if system_prompt.is_some() {
                engine_config.system_prompt = system_prompt;
            }
            if append_system_prompt.is_some() {
                engine_config.append_system_prompt = append_system_prompt;
            }
            if let Some(custom_instructions) = plan_mode_custom_instructions {
                engine_config.plan_mode_settings.custom_instructions = custom_instructions;
            }
            if requires_structured_output {
                engine_config.requires_structured_output = true;
            }
        })
        .await;

        if permission_mode.is_none() && !config.agent_progress_summaries_enabled {
            return;
        }

        let mut app_state = self.engine_state_resources.app_state().write().await;
        if let Some(mode) = permission_mode {
            // Brand-new session: the engine config / rules are not part of a
            // turn build yet, so the Auto-entry stash starts empty. The
            // evaluator-facing strip in ToolContextFactory::build, keyed on
            // live mode==Auto, is the runtime guard once a turn starts.
            let live_allow_rules = coco_types::PermissionRulesBySource::new();
            let previous = app_state
                .permissions
                .mode
                .unwrap_or(coco_types::PermissionMode::Default);
            coco_permissions::apply_permission_mode_transition_to_app_state(
                &mut app_state,
                previous,
                mode,
                &live_allow_rules,
                coco_permissions::PlanModeAutoOptions::default(),
            );
        }
        if config.agent_progress_summaries_enabled {
            app_state.agent_progress_summaries_enabled = true;
        }
    }
    pub async fn apply_turn_runtime_config(&self, config: super::SessionTurnRuntimeConfig) {
        self.update_engine_config(move |engine_config| {
            engine_config.is_non_interactive = config.is_non_interactive;
            engine_config.avoid_permission_prompts = config.avoid_permission_prompts;
            engine_config.permission_mode = config.permission_mode;
            engine_config.permission_mode_availability = config.permission_mode_availability;
            engine_config.permission_rule_source_roots = config.permission_rule_source_roots;
            engine_config.max_turns = config.max_turns;
            engine_config.total_token_budget = config.total_token_budget;
            engine_config.cwd_override = config.cwd_override;
            engine_config.tool_filter = config.tool_filter;
            engine_config.plans_directory = config.plans_directory;
            engine_config.plan_mode_settings.custom_instructions =
                config.plan_mode_custom_instructions;
        })
        .await;
    }
    pub async fn seed_todo_list_snapshot(&self, key: String, items: Vec<coco_types::TodoRecord>) {
        let handle = self.handle_resources.todo_list.read().await.clone();
        handle.write(&key, items.clone()).await;
        let mut app_state = self.engine_state_resources.app_state().write().await;
        if items.is_empty() {
            app_state.todos_by_agent.remove(&key);
        } else {
            app_state.todos_by_agent.insert(key, items);
        }
    }
    pub async fn set_agent_color(&self, color: Option<coco_types::AgentColorName>) {
        self.engine_state_resources
            .app_state()
            .write()
            .await
            .agent_color = color;
    }
    pub async fn todo_list_snapshot(&self, key: &str) -> Vec<coco_types::TodoRecord> {
        self.handle_resources.todo_list.read().await.read(key).await
    }
}
