use coco_types::ModelRole;
use coco_types::ModelSpec;
use coco_types::ProviderModelSelection;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;

use super::SessionRuntime;

/// In-memory binding for a single [`ModelRole`] that overrides the
/// `RuntimeConfig.model_roles` entry for the lifetime of one session.
/// Populated by the TUI model picker (`UserCommand::SetModelRole` ->
/// [`SessionRuntime::apply_role_override`]) and Ctrl+T thinking cycle
/// (`UserCommand::SetThinkingLevel` ->
/// [`SessionRuntime::apply_role_effort`]). The picker carries an
/// explicit `effort`; Ctrl+T preserves the spec and only changes
/// `effort`.
#[derive(Debug, Clone)]
pub struct RoleOverride {
    /// `(provider, model_id, display_name, api)` for the role.
    pub spec: ModelSpec,
    /// User's explicit effort choice. `None` => engine reaches for the
    /// model's `default_thinking_level` (or provider default if the
    /// model doesn't declare one).
    pub effort: Option<ReasoningEffort>,
}

/// Construct a [`ThinkingLevel`] for an effort, threading the current
/// model's declared `supported_thinking_levels` budget/options when
/// available. Falls back to a budget-less level so provider-specific
/// conversion can pick defaults for models without metadata.
pub(crate) fn thinking_level_for_effort_from(
    model_info: Option<&coco_config::ModelInfo>,
    effort: ReasoningEffort,
) -> ThinkingLevel {
    let requested = ThinkingLevel {
        effort,
        budget_tokens: None,
        options: std::collections::HashMap::new(),
    };
    model_info
        .map(|info| info.resolve_thinking_level(&requested))
        .unwrap_or(requested)
}

fn resolve_model_info(
    runtime_config: &coco_config::RuntimeConfig,
    provider: &str,
    model_id: &str,
) -> Option<coco_config::ModelInfo> {
    runtime_config
        .model_registry
        .resolve(provider, model_id)
        .map(|resolved| resolved.info.clone())
}

pub(crate) fn resolve_model_selection_from_runtime_config(
    runtime_config: &coco_config::RuntimeConfig,
    raw_model: &str,
) -> Option<ProviderModelSelection> {
    let raw_model = raw_model.trim();
    if raw_model.is_empty() {
        return None;
    }

    if let Ok(selection) = ProviderModelSelection::from_slash_str(raw_model)
        && ((selection.provider == coco_config::MOA_PROVIDER
            && runtime_config
                .model_roles
                .moa_preset(&selection.model_id)
                .is_some())
            || runtime_config
                .model_registry
                .resolve(&selection.provider, &selection.model_id)
                .is_some())
    {
        return Some(selection);
    }

    if let Some(main) = runtime_config.model_roles.get(ModelRole::Main)
        && runtime_config
            .model_registry
            .resolve(&main.provider, raw_model)
            .is_some()
    {
        return Some(ProviderModelSelection {
            provider: main.provider.clone(),
            model_id: raw_model.to_string(),
        });
    }

    runtime_config.providers.keys().find_map(|provider| {
        runtime_config
            .model_registry
            .resolve(provider, raw_model)
            .map(|_| ProviderModelSelection {
                provider: provider.clone(),
                model_id: raw_model.to_string(),
            })
    })
}

impl SessionRuntime {
    /// Resolve a role to `(spec, effort)`, layering overrides above
    /// `runtime_config.model_roles`. Returns `None` only when the role
    /// is not configured anywhere (model picker / engine consumers
    /// already guard on this via the Main fallback chain).
    /// Used by [`Self::current_engine_config`] to project the active
    /// Main effort onto `QueryEngineConfig.thinking_level`.
    pub async fn resolve_role(&self, role: ModelRole) -> Option<RoleOverride> {
        {
            let overrides = self.engine_config_resources.role_overrides().read().await;
            if let Some(ov) = overrides.get(&role) {
                return Some(ov.clone());
            }
        }
        // No session override -> seed from the configured role slot,
        // carrying its per-slot effort (`models.<role>.effort`) so
        // `/status` and the picker reflect the effort that will actually
        // apply. A role defaulted to Main at config time has its effort
        // stripped, so this yields `None` there (model default applies).
        if let Some(slot) = self.runtime_config().model_roles.primary_slot(role) {
            return Some(RoleOverride {
                spec: slot.model.clone(),
                effort: slot.effort,
            });
        }
        // Role absent entirely (pre-defaulting edge) - borrow Main's
        // model with no effort.
        self.runtime_config()
            .model_roles
            .get(role)
            .map(|spec| RoleOverride {
                spec: spec.clone(),
                effort: None,
            })
    }

    /// Install (or replace) an in-memory override for `role`. The
    /// override layers above `runtime_config.model_roles` and is NOT
    /// persisted to the global config file - re-bind on every session via the
    /// picker, or edit settings to make the change durable.
    /// For `role == Main` this also rewrites the model-sensitive
    /// `engine_config` fields so UI, compaction, and tool-context mirrors see
    /// the new identity on the next turn.
    pub async fn apply_role_override(
        &self,
        role: ModelRole,
        ov: RoleOverride,
    ) -> anyhow::Result<()> {
        let effort = ov.effort;
        let spec = ov.spec.clone();
        let model_id = ov.spec.model_id.clone();

        if role == ModelRole::Main {
            // Main's session effort rides `engine_config.thinking_level`
            // (layer 1, main-loop only), NOT the shared Main client that
            // forks inherit - so pass `None` here to keep the slot
            // effort-less. This is the fork-isolation guarantee: a picker
            // or Ctrl+T effort on Main never leaks into fork spawns.
            self.execution
                .model_runtimes()
                .rebind_role_primary(role, spec, /*effort*/ None)
                .map_err(anyhow::Error::from)?;
            let model_info =
                resolve_model_info(self.runtime_config(), &ov.spec.provider, &ov.spec.model_id);
            // Store the override only after the registry accepted the
            // replacement runtime.
            {
                let mut overrides = self.engine_config_resources.role_overrides().write().await;
                overrides.insert(role, ov);
            }
            self.update_engine_config(move |cfg| {
                cfg.model_id = model_id;
                cfg.thinking_level =
                    effort.map(|e| thinking_level_for_effort_from(model_info.as_ref(), e));
            })
            .await;
            return Ok(());
        }

        // Non-Main roles feed subagents through the role runtime's client,
        // so the picker's effort binds to the rebuilt slot (layer 2). This
        // is safe to isolate - each non-Main role owns its own client, not
        // shared with the main loop or forks.
        let spec = ov.spec.clone();
        self.execution
            .model_runtimes()
            .rebind_role_primary(role, spec, ov.effort)
            .map_err(anyhow::Error::from)?;
        let mut overrides = self.engine_config_resources.role_overrides().write().await;
        overrides.insert(role, ov);
        Ok(())
    }

    /// Update only the `effort` on an existing role override, preserving
    /// the spec. When the role has no prior override, the current
    /// `runtime_config.model_roles` spec is captured and stored
    /// alongside the new effort so subsequent reads see a consistent
    /// `RoleOverride`.
    ///
    /// The effort's live landing site differs by role - same rule as
    /// [`Self::apply_role_override`]:
    /// - **Main**: rewrites `engine_config.thinking_level` (layer 1,
    ///   main-loop only). Never bound to the Main slot client - forks
    ///   share it, and they take their thinking from the
    ///   `CacheSafeParams.effort` parity snapshot instead.
    /// - **Non-Main**: rebinds the role runtime so the effort lands on
    ///   the rebuilt slot client (layer 2), where that role's subagents
    ///   actually read it. Without the rebind the override map updates
    ///   but the wire never changes.
    pub async fn apply_role_effort(&self, role: ModelRole, effort: Option<ReasoningEffort>) {
        let spec_for_seed = self.runtime_config().model_roles.get(role).cloned();
        let effective_spec = self
            .engine_config_resources
            .role_overrides()
            .read()
            .await
            .get(&role)
            .map(|ov| ov.spec.clone())
            .or_else(|| spec_for_seed.clone());
        let mut overrides = self.engine_config_resources.role_overrides().write().await;
        match overrides.get_mut(&role) {
            Some(existing) => existing.effort = effort,
            None => {
                if let Some(spec) = spec_for_seed {
                    overrides.insert(role, RoleOverride { spec, effort });
                }
            }
        }
        drop(overrides);
        if role == ModelRole::Main {
            let model_info = effective_spec.as_ref().and_then(|spec| {
                resolve_model_info(self.runtime_config(), &spec.provider, &spec.model_id)
            });
            self.update_engine_config(|cfg| {
                cfg.thinking_level =
                    effort.map(|e| thinking_level_for_effort_from(model_info.as_ref(), e));
            })
            .await;
        } else if let Some(spec) = effective_spec
            && let Err(error) = self
                .execution
                .model_runtimes()
                .rebind_role_primary(role, spec, effort)
        {
            tracing::warn!(
                role = %role.as_str(),
                error = %error,
                "apply_role_effort: role runtime rebind failed; effort override stored but not live",
            );
        }
    }

    /// Render a live `/status` report from the session runtime. Replaces the
    /// former all-hardcoded-placeholder output with real values: the resolved
    /// Main/Fast model roles, permission mode, thinking level, plan-mode gate,
    /// and connected MCP servers. (Plugin count is intentionally omitted - the
    /// runtime holds no persistent plugin manager to read without a reload.)
    pub async fn status_report(&self) -> String {
        use std::fmt::Write as _;

        let cfg = self.current_engine_config().await;
        let mut out = String::from("Session status:\n");
        let _ = writeln!(out, "  Version: {}", env!("CARGO_PKG_VERSION"));

        // Effective Main effort = explicit per-call level (Ctrl+T, layer 1)
        // or the resolved Main slot / override effort (layer 2). `None`
        // means "defer to the model default", which is NOT the same as
        // "off" - so render it as the model default rather than lying.
        let main_role = self.resolve_role(ModelRole::Main).await;
        if let Some(main) = main_role.as_ref() {
            let _ = writeln!(
                out,
                "  Model: {} ({})",
                main.spec.model_id, main.spec.provider
            );
        }
        let _ = writeln!(out, "  Permission mode: {:?}", cfg.permission_mode);
        let effective_effort = cfg
            .thinking_level
            .as_ref()
            .map(|t| t.effort)
            .or_else(|| main_role.as_ref().and_then(|m| m.effort));
        match effective_effort {
            Some(effort) => {
                let _ = writeln!(out, "  Thinking: {effort}");
            }
            None => {
                let _ = writeln!(out, "  Thinking: model default");
            }
        }
        let _ = writeln!(
            out,
            "  Plan mode: {}",
            if cfg.plan_mode_required { "on" } else { "off" }
        );
        if let Some(fast) = self.resolve_role(ModelRole::Fast).await {
            let _ = writeln!(
                out,
                "  Fast model: {} ({})",
                fast.spec.model_id, fast.spec.provider
            );
        }

        let servers = match self.current_mcp_handle().await {
            Some(handle) => handle.connected_servers().await,
            None => Vec::new(),
        };
        if servers.is_empty() {
            let _ = write!(out, "  MCP servers: none connected");
        } else {
            let _ = write!(
                out,
                "  MCP servers: {} connected ({})",
                servers.len(),
                servers.join(", ")
            );
        }
        out
    }
}
