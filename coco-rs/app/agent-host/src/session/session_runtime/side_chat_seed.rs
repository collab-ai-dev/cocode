use std::sync::Arc;

use coco_context::side_chat::BoundedContext;
use coco_types::LiveToolPermissionState;

/// Owned, already-resolved inputs for constructing one sidechat child.
///
/// The seed owns the resolved parent configuration and catalogs needed by the
/// child, so child construction never refolds settings from disk after history
/// capture. Mutable parent state is copied, not shared.
pub struct SideChatSeed {
    pub(crate) context: BoundedContext,
    pub(crate) runtime_config: Arc<coco_config::RuntimeConfig>,
    pub(crate) engine_config: coco_query::QueryEngineConfig,
    pub(crate) permissions: LiveToolPermissionState,
    pub(crate) model_runtimes: Arc<coco_inference::ModelRuntimeRegistry>,
    pub(crate) tools: Arc<coco_tool_runtime::ToolRegistry>,
    pub(crate) command_registry: Arc<coco_commands::CommandRegistry>,
    pub(crate) skill_manager: Arc<coco_skills::SkillManager>,
    pub(crate) project_services: Arc<coco_app_runtime::ProjectServices>,
    pub(crate) parent_usage_accounting: coco_query::usage_accounting::UsageAccounting,
}

impl SideChatSeed {
    /// Returns the immutable parent-context snapshot inherited by the child.
    pub fn context(&self) -> &BoundedContext {
        &self.context
    }
}
