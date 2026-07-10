use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_session::SessionManager;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_types::ModelSpec;
use coco_types::PermissionMode;
use coco_types::PermissionModeAvailability;
use coco_types::SessionId;
use tokio::sync::RwLock;

use super::SessionHandle;
use super::SessionRuntimeBuildOpts;
use crate::Cli;
use crate::headless::build_runtime_config_with_reloader;
use crate::process_runtime::ProcessRuntime;
use crate::project_services::ProjectServices;
use crate::session_bootstrap::build_engine_resources;

/// Owned construction inputs for one family of session runtimes.
///
/// AppServer bridge owner tasks pass this through `spawn_load` / replace while
/// entry points retain their surface-specific late-binds after the handle is
/// built.
#[derive(Clone)]
pub struct SessionRuntimeFactory {
    opts: Arc<SessionRuntimeFactoryOpts>,
}

pub struct SessionRuntimeBootstrap {
    pub runtime_config: Arc<RuntimeConfig>,
    pub model_id: String,
    pub system_prompt: String,
    pub permission_mode_availability: PermissionModeAvailability,
    pub permission_mode: PermissionMode,
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    pub skill_manager: Arc<coco_skills::SkillManager>,
    pub project_services: Arc<ProjectServices>,
    pub agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
}

#[derive(Clone)]
pub struct SessionRuntimeBootstrapSource {
    kind: SessionRuntimeBootstrapSourceKind,
}

#[derive(Clone)]
enum SessionRuntimeBootstrapSourceKind {
    StartupSnapshot(Arc<SessionRuntimeBootstrap>),
    PerSessionFold {
        cli: Arc<Cli>,
        process_runtime: Arc<ProcessRuntime>,
    },
}

struct SessionRuntimeBootstrapBuild {
    bootstrap: Arc<SessionRuntimeBootstrap>,
    config_reloader: Option<coco_config_reload::RuntimeReloader>,
}

impl SessionRuntimeBootstrapSource {
    /// Compatibility source for the current process-bootstrap session
    /// bootstrap fold.
    ///
    /// New session construction routes through this type so the future
    /// per-session fold can replace this method without touching every runtime
    /// builder call site.
    pub fn startup_snapshot(bootstrap: SessionRuntimeBootstrap) -> Self {
        Self {
            kind: SessionRuntimeBootstrapSourceKind::StartupSnapshot(Arc::new(bootstrap)),
        }
    }

    /// Build a fresh config/resource bundle for every target session cwd.
    ///
    /// This is the migration seam for the final per-session config fold:
    /// `RuntimeConfig` and all config-derived engine resources are rebuilt as
    /// one coherent bundle for the session being constructed.
    pub fn per_session_fold(cli: Arc<Cli>, process_runtime: Arc<ProcessRuntime>) -> Self {
        Self {
            kind: SessionRuntimeBootstrapSourceKind::PerSessionFold {
                cli,
                process_runtime,
            },
        }
    }

    fn bootstrap_for_session(
        &self,
        cwd: &Path,
        _session_id_override: Option<&SessionId>,
    ) -> Result<SessionRuntimeBootstrapBuild> {
        match &self.kind {
            SessionRuntimeBootstrapSourceKind::StartupSnapshot(snapshot) => {
                Ok(SessionRuntimeBootstrapBuild {
                    bootstrap: Arc::clone(snapshot),
                    config_reloader: None,
                })
            }
            SessionRuntimeBootstrapSourceKind::PerSessionFold {
                cli,
                process_runtime,
            } => {
                let (config_reloader, runtime_config) =
                    build_runtime_config_with_reloader(cli, cwd)?;
                let runtime_config = Arc::new(runtime_config);
                let resources = build_engine_resources(process_runtime, cli, &runtime_config, cwd)?;
                let config_home = coco_config::global_config::config_home();
                let bootstrap = SessionRuntimeBootstrap {
                    runtime_config,
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
    }
}

pub struct SessionRuntimeFactoryOpts {
    pub cli: Arc<Cli>,
    pub bootstrap_source: SessionRuntimeBootstrapSource,
    pub cwd: PathBuf,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub tools: Arc<ToolRegistry>,
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
        let SessionRuntimeBootstrapBuild {
            bootstrap,
            config_reloader,
        } = opts
            .bootstrap_source
            .bootstrap_for_session(&cwd, session_id_override.as_ref())?;
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
            tools: Arc::clone(&opts.tools),
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
