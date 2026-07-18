use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use coco_session::SessionManager;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_types::ModelSpec;
use coco_types::PermissionModeAvailability;
use coco_types::SessionId;

use super::SessionHandle;
use super::SessionRuntimeBuildOpts;
use crate::AgentHostOptions;
use crate::headless::build_runtime_config_with_reloader_roots;
use crate::session_bootstrap::build_engine_resources;
use coco_app_runtime::BootstrapError;
use coco_app_runtime::BootstrapSource;
use coco_app_runtime::PrebuiltBootstrapSource;
use coco_app_runtime::ProcessRuntime;
use coco_app_runtime::SessionRuntimeBootstrap;
use coco_app_runtime::SessionRuntimeBootstrapBuild;

/// Owned construction inputs for one family of session runtimes.
///
/// AppServer bridge owner tasks pass this through `spawn_load` / replace while
/// entry points retain their surface-specific late-binds after the handle is
/// built.
#[derive(Clone)]
pub struct SessionRuntimeFactory {
    opts: Arc<SessionRuntimeFactoryOpts>,
}

/// The production per-session config fold: rebuilds `RuntimeConfig` and all
/// config-derived engine resources for each target cwd. AgentHostOptions-coupled, so it
/// stays in `coco-agent-host` and implements the `coco-app-runtime` `BootstrapSource`
/// trait, converting its `anyhow` failures to the Tier-3 `BootstrapError` at
/// the crate boundary.
struct PerSessionFoldSource {
    cli: Arc<AgentHostOptions>,
    process_runtime: Arc<ProcessRuntime>,
}

impl BootstrapSource for PerSessionFoldSource {
    fn bootstrap_for_session(
        &self,
        cwd: &Path,
        _session_id_override: Option<&SessionId>,
    ) -> Result<SessionRuntimeBootstrapBuild, BootstrapError> {
        let session_workspace = coco_app_runtime::SessionWorkspace::resolve(cwd.to_path_buf());
        let (config_reloader, runtime_config) = build_runtime_config_with_reloader_roots(
            &self.cli,
            &session_workspace.project_root,
            &session_workspace.cwd,
        )
        .map_err(|e| BootstrapError::fold(format!("{e:#}")))?;
        let runtime_config = Arc::new(runtime_config);
        let resources =
            build_engine_resources(&self.process_runtime, &self.cli, &runtime_config, cwd)
                .map_err(|e| BootstrapError::fold(format!("{e:#}")))?;
        let config_home = coco_config::global_config::config_home();
        let bootstrap = SessionRuntimeBootstrap {
            runtime_config,
            tools: resources.tools,
            model_id: resources.model_id,
            system_prompt: resources.system_prompt,
            permission_mode_availability: PermissionModeAvailability::new(
                resources.startup.bypass_available,
                resources.startup.auto_available,
            ),
            permission_mode: resources.startup.mode,
            command_registry: resources.command_registry,
            skill_manager: resources.skill_manager,
            agent_search_paths: resources
                .project_services
                .agent_search_paths(&config_home, cwd),
            project_services: resources.project_services,
        };
        Ok(SessionRuntimeBootstrapBuild {
            bootstrap: Arc::new(bootstrap),
            config_reloader,
        })
    }
}

/// Cheap, cloneable handle to a [`BootstrapSource`] implementation.
#[derive(Clone)]
pub struct SessionRuntimeBootstrapSource {
    source: Arc<dyn BootstrapSource>,
}

impl SessionRuntimeBootstrapSource {
    pub fn from_source(source: Arc<dyn BootstrapSource>) -> Self {
        Self { source }
    }

    /// Source backed by an already-resolved bundle, bypassing the per-session
    /// config fold. Used by tests and embedders that build a runtime from
    /// explicit inputs instead of resolving config from disk.
    pub fn from_prebuilt_bootstrap(bootstrap: SessionRuntimeBootstrap) -> Self {
        Self {
            source: Arc::new(PrebuiltBootstrapSource::new(bootstrap)),
        }
    }

    /// Source that runs the production per-session config fold for each target
    /// cwd, rebuilding `RuntimeConfig` and all config-derived engine resources.
    pub fn per_session_fold(
        cli: Arc<AgentHostOptions>,
        process_runtime: Arc<ProcessRuntime>,
    ) -> Self {
        Self {
            source: Arc::new(PerSessionFoldSource {
                cli,
                process_runtime,
            }),
        }
    }

    fn bootstrap_for_session(
        &self,
        cwd: &Path,
        session_id_override: Option<&SessionId>,
    ) -> Result<SessionRuntimeBootstrapBuild, BootstrapError> {
        self.source.bootstrap_for_session(cwd, session_id_override)
    }
}

pub struct SessionRuntimeFactoryOpts {
    pub cli: Arc<AgentHostOptions>,
    pub bootstrap_source: SessionRuntimeBootstrapSource,
    pub cwd: PathBuf,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    pub process_runtime: Arc<ProcessRuntime>,
    pub builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    pub is_non_interactive: bool,
}

pub struct SessionRuntimeFactoryHostConfig {
    pub cli: Arc<AgentHostOptions>,
    pub cwd: PathBuf,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    pub process_runtime: Arc<ProcessRuntime>,
    pub builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    pub is_non_interactive: bool,
}

impl SessionRuntimeFactory {
    pub fn new(opts: SessionRuntimeFactoryOpts) -> Self {
        Self {
            opts: Arc::new(opts),
        }
    }

    pub fn from_host_config(config: SessionRuntimeFactoryHostConfig) -> Self {
        Self::new(SessionRuntimeFactoryOpts {
            cli: Arc::clone(&config.cli),
            bootstrap_source: SessionRuntimeBootstrapSource::per_session_fold(
                Arc::clone(&config.cli),
                Arc::clone(&config.process_runtime),
            ),
            cwd: config.cwd,
            model_runtimes: config.model_runtimes,
            session_manager: config.session_manager,
            fast_model_spec: config.fast_model_spec,
            permission_bridge: config.permission_bridge,
            process_runtime: config.process_runtime,
            builtin_agent_catalog: config.builtin_agent_catalog,
            is_non_interactive: config.is_non_interactive,
        })
    }

    pub async fn build_fresh(&self) -> Result<SessionHandle> {
        self.build(None, Default::default()).await
    }

    pub async fn build_with_session_id(
        &self,
        session_id: SessionId,
        callback_requirements: coco_types::SessionCallbackRequirements,
    ) -> Result<SessionHandle> {
        self.build(Some(session_id), callback_requirements).await
    }

    pub async fn build(
        &self,
        session_id_override: Option<SessionId>,
        callback_requirements: coco_types::SessionCallbackRequirements,
    ) -> Result<SessionHandle> {
        let cwd = self.opts.cwd.clone();
        self.build_for_cwd(session_id_override, cwd, callback_requirements)
            .await
    }

    pub async fn build_with_session_id_and_cwd(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
        callback_requirements: coco_types::SessionCallbackRequirements,
    ) -> Result<SessionHandle> {
        self.build_for_cwd(Some(session_id), cwd, callback_requirements)
            .await
    }

    pub async fn build_for_cwd(
        &self,
        session_id_override: Option<SessionId>,
        cwd: PathBuf,
        callback_requirements: coco_types::SessionCallbackRequirements,
    ) -> Result<SessionHandle> {
        self.build_with_profile(
            session_id_override,
            cwd,
            callback_requirements,
            super::SessionExecutionProfile::Primary,
        )
        .await
    }

    /// Build an ephemeral, read-only sidechat child runtime. Same construction
    /// path as [`Self::build_for_cwd`], but with the `SideChatReadOnly`
    /// execution profile: no durable/background ownership, and the structural
    /// read-only tool boundary installed on every turn.
    ///
    /// The child's initial history is seeded with `inherited` bounded parent
    /// context followed by the read-only boundary fragment, so the model sees
    /// the parent conversation as reference material immediately before the
    /// first question. The seed is appended silently (no UI emit); the child's
    /// visual scrollback starts at the boundary.
    pub async fn build_side_chat(
        &self,
        session_id_override: Option<SessionId>,
        cwd: PathBuf,
        callback_requirements: coco_types::SessionCallbackRequirements,
        seed: super::SideChatSeed,
    ) -> Result<SessionHandle> {
        use coco_context::side_chat::ContextualUserFragment;

        let super::SideChatSeed {
            context,
            runtime_config,
            engine_config,
            permissions,
            model_runtimes,
            tools,
            command_registry,
            skill_manager,
            project_services,
            parent_usage_accounting,
        } = seed;
        let opts = self.opts.as_ref();
        let handle = SessionHandle::build(SessionRuntimeBuildOpts {
            cli: opts.cli.as_ref(),
            runtime_config,
            // A sidechat is a point-in-time child. It must not drift away from
            // the captured parent through a separate settings watcher.
            config_reloader: None,
            cwd,
            model_id: engine_config.model_id.clone(),
            system_prompt: engine_config.system_prompt.clone().unwrap_or_default(),
            permission_mode_availability: engine_config.permission_mode_availability,
            permission_mode: engine_config.permission_mode,
            model_runtimes: Some(model_runtimes),
            tools,
            session_manager: Arc::clone(&opts.session_manager),
            fast_model_spec: opts.fast_model_spec.clone(),
            permission_bridge: opts.permission_bridge.clone(),
            command_registry: Arc::new(tokio::sync::RwLock::new(command_registry)),
            skill_manager,
            project_services,
            process_runtime: Arc::clone(&opts.process_runtime),
            agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::noninteractive(),
            session_id_override,
            is_non_interactive: opts.is_non_interactive,
            execution_profile: super::SessionExecutionProfile::SideChatReadOnly,
            callback_requirements,
        })
        .await?;
        handle
            .apply_side_chat_parent_state(engine_config, permissions)
            .await;
        handle
            .install_usage_mirror(parent_usage_accounting, coco_types::UsageSource::SideQuery)
            .await;

        let mut inherited_messages = context.into_messages();
        inherited_messages.push(std::sync::Arc::new(coco_messages::create_meta_message(
            &coco_context::side_chat::SideChatBoundaryFragment.render(),
        )));
        handle
            .append_arc_messages_to_history_and_snapshot(inherited_messages)
            .await;
        Ok(handle)
    }

    async fn build_with_profile(
        &self,
        session_id_override: Option<SessionId>,
        cwd: PathBuf,
        callback_requirements: coco_types::SessionCallbackRequirements,
        execution_profile: super::SessionExecutionProfile,
    ) -> Result<SessionHandle> {
        let opts = self.opts.as_ref();
        // Run the disk-heavy per-session fold (settings reads + plugin scan)
        // off the async runtime so it can't stall other tasks.
        let source = opts.bootstrap_source.clone();
        let fold_cwd = cwd.clone();
        let fold_override = session_id_override.clone();
        let SessionRuntimeBootstrapBuild {
            bootstrap,
            config_reloader,
        } = tokio::task::spawn_blocking(move || {
            source.bootstrap_for_session(&fold_cwd, fold_override.as_ref())
        })
        .await
        .map_err(|error| anyhow::anyhow!("session bootstrap fold task failed: {error}"))??;
        SessionHandle::build(SessionRuntimeBuildOpts {
            cli: opts.cli.as_ref(),
            runtime_config: Arc::clone(&bootstrap.runtime_config),
            config_reloader,
            cwd,
            model_id: bootstrap.model_id.clone(),
            system_prompt: bootstrap.system_prompt.clone(),
            permission_mode_availability: bootstrap.permission_mode_availability,
            permission_mode: bootstrap.permission_mode,
            model_runtimes: opts.model_runtimes.clone(),
            // Per-session registry from this session's fold, not a shared
            // process-wide one.
            tools: Arc::clone(&bootstrap.tools),
            session_manager: Arc::clone(&opts.session_manager),
            fast_model_spec: opts.fast_model_spec.clone(),
            permission_bridge: opts.permission_bridge.clone(),
            command_registry: Arc::clone(&bootstrap.command_registry),
            skill_manager: Arc::clone(&bootstrap.skill_manager),
            project_services: Arc::clone(&bootstrap.project_services),
            process_runtime: Arc::clone(&opts.process_runtime),
            agent_search_paths: bootstrap.agent_search_paths.clone(),
            builtin_agent_catalog: opts.builtin_agent_catalog,
            session_id_override,
            is_non_interactive: opts.is_non_interactive,
            execution_profile,
            callback_requirements,
        })
        .await
    }
}
