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
use crate::process_runtime::ProcessRuntime;
use crate::project_services::ProjectServices;

/// Owned construction inputs for one family of session runtimes.
///
/// This is the compatibility shape that AppServer owner tasks can later hold
/// and call from `spawn_load` / replace. Entry points still own their
/// surface-specific late-binds after the handle is built.
#[derive(Clone)]
pub struct SessionRuntimeFactory {
    opts: Arc<SessionRuntimeFactoryOpts>,
}

pub struct SessionRuntimeFactoryOpts {
    pub cli: Arc<Cli>,
    pub runtime_config: Arc<RuntimeConfig>,
    pub cwd: PathBuf,
    pub model_id: String,
    pub system_prompt: String,
    pub permission_mode_availability: PermissionModeAvailability,
    pub permission_mode: PermissionMode,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub tools: Arc<ToolRegistry>,
    pub session_manager: Arc<SessionManager>,
    pub fast_model_spec: Option<ModelSpec>,
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    pub skill_manager: Arc<coco_skills::SkillManager>,
    pub project_services: Arc<ProjectServices>,
    pub process_runtime: Arc<ProcessRuntime>,
    pub agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
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
        let opts = self.opts.as_ref();
        SessionHandle::build(SessionRuntimeBuildOpts {
            cli: opts.cli.as_ref(),
            runtime_config: Arc::clone(&opts.runtime_config),
            cwd: opts.cwd.clone(),
            model_id: opts.model_id.clone(),
            system_prompt: opts.system_prompt.clone(),
            permission_mode_availability: opts.permission_mode_availability,
            permission_mode: opts.permission_mode,
            model_runtimes: opts.model_runtimes.clone(),
            tools: Arc::clone(&opts.tools),
            session_manager: Arc::clone(&opts.session_manager),
            fast_model_spec: opts.fast_model_spec.clone(),
            permission_bridge: opts.permission_bridge.clone(),
            command_registry: Arc::clone(&opts.command_registry),
            skill_manager: Arc::clone(&opts.skill_manager),
            project_services: Arc::clone(&opts.project_services),
            process_runtime: Arc::clone(&opts.process_runtime),
            agent_search_paths: opts.agent_search_paths.clone(),
            builtin_agent_catalog: opts.builtin_agent_catalog,
            session_id_override,
            is_non_interactive: opts.is_non_interactive,
        })
        .await
    }
}
