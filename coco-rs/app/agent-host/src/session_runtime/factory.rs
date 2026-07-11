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
use coco_app_runtime::ProcessRuntime;
use coco_app_runtime::SessionRuntimeBootstrap;
use coco_app_runtime::SessionRuntimeBootstrapBuild;
use coco_app_runtime::StartupSnapshotSource;

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
    /// Source backed by a pre-built bundle (tests / legacy startup snapshot).
    pub fn startup_snapshot(bootstrap: SessionRuntimeBootstrap) -> Self {
        Self {
            source: Arc::new(StartupSnapshotSource::new(bootstrap)),
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

impl SessionRuntimeFactory {
    pub fn new(opts: SessionRuntimeFactoryOpts) -> Self {
        Self {
            opts: Arc::new(opts),
        }
    }

    pub async fn build_fresh(&self) -> Result<SessionHandle> {
        self.build(None).await
    }

    pub async fn build_with_session_id(&self, session_id: SessionId) -> Result<SessionHandle> {
        self.build(Some(session_id)).await
    }

    pub async fn build(&self, session_id_override: Option<SessionId>) -> Result<SessionHandle> {
        let cwd = self.opts.cwd.clone();
        self.build_for_cwd(session_id_override, cwd).await
    }

    pub async fn build_with_session_id_and_cwd(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
    ) -> Result<SessionHandle> {
        self.build_for_cwd(Some(session_id), cwd).await
    }

    pub async fn build_for_cwd(
        &self,
        session_id_override: Option<SessionId>,
        cwd: PathBuf,
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
        })
        .await
    }
}
