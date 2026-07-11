//! Session construction bundle: the fully-resolved, config-derived inputs
//! needed to build one `SessionRuntime`.
//!
//! Owned here (rather than in `coco-cli`) because this is the output of the
//! per-session config fold and the input to session-runtime construction —
//! both of which are moving behind this crate boundary. The producer of the
//! bundle (the per-session fold, still Cli-coupled) stays in `coco-cli` for
//! now; see the A3 migration notes in `multi-session-app-server-plan.md`.

use std::path::Path;
use std::sync::Arc;

use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use coco_tool_runtime::ToolRegistry;
use coco_types::PermissionMode;
use coco_types::PermissionModeAvailability;
use coco_types::SessionId;
use snafu::Snafu;
use tokio::sync::RwLock;

use crate::ProjectServices;

/// The fully-resolved inputs for constructing one session runtime.
///
/// Every field is a config-derived or process/project-scoped resource; none
/// is per-turn mutable state. A fresh bundle is produced per `session/start`
/// (and per resume) by the config fold.
pub struct SessionRuntimeBootstrap {
    pub runtime_config: Arc<RuntimeConfig>,
    /// The session's own tool registry, built by the per-session fold. Each
    /// session gets a fresh registry so one session's MCP/plugin reload cannot
    /// mutate another session's tool set mid-turn.
    pub tools: Arc<ToolRegistry>,
    pub model_id: String,
    pub system_prompt: String,
    pub permission_mode_availability: PermissionModeAvailability,
    pub permission_mode: PermissionMode,
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    pub skill_manager: Arc<coco_skills::SkillManager>,
    pub project_services: Arc<ProjectServices>,
    pub agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
}

/// The output of a [`BootstrapSource`]: the resolved bundle plus the optional
/// config reloader that watches the session's settings files.
pub struct SessionRuntimeBootstrapBuild {
    pub bootstrap: Arc<SessionRuntimeBootstrap>,
    pub config_reloader: Option<coco_config_reload::RuntimeReloader>,
}

/// Error raised while producing a session bootstrap bundle.
///
/// Cli-coupled folds in `coco-cli` convert their `anyhow` failures into this
/// Tier-3 error at the crate boundary via [`BootstrapError::fold`].
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum BootstrapError {
    #[snafu(display("{message}"))]
    Fold {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl BootstrapError {
    pub fn fold(message: impl Into<String>) -> Self {
        FoldSnafu {
            message: message.into(),
        }
        .build()
    }
}

impl ErrorExt for BootstrapError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Fold { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Produces the [`SessionRuntimeBootstrap`] bundle for a given session cwd.
///
/// This is the seam that lets the config-fold implementation vary while the
/// session runtime factory stays fixed. The production per-session fold is
/// Cli-coupled and lives in `coco-cli`; [`StartupSnapshotSource`] (a pre-built
/// bundle, used by tests and legacy startup) is source-independent.
pub trait BootstrapSource: Send + Sync {
    fn bootstrap_for_session(
        &self,
        cwd: &Path,
        session_id_override: Option<&SessionId>,
    ) -> Result<SessionRuntimeBootstrapBuild, BootstrapError>;
}

/// A pre-built bundle (no per-session fold). Used by tests and the legacy
/// startup-snapshot path.
pub struct StartupSnapshotSource {
    bootstrap: Arc<SessionRuntimeBootstrap>,
}

impl StartupSnapshotSource {
    pub fn new(bootstrap: SessionRuntimeBootstrap) -> Self {
        Self {
            bootstrap: Arc::new(bootstrap),
        }
    }
}

impl BootstrapSource for StartupSnapshotSource {
    fn bootstrap_for_session(
        &self,
        _cwd: &Path,
        _session_id_override: Option<&SessionId>,
    ) -> Result<SessionRuntimeBootstrapBuild, BootstrapError> {
        Ok(SessionRuntimeBootstrapBuild {
            bootstrap: Arc::clone(&self.bootstrap),
            config_reloader: None,
        })
    }
}
