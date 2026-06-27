//! Fluent builders for the two test fixtures that integration tests construct
//! by hand most often: a [`ToolRegistry`] populated with built-in tools, and a
//! [`ToolPermissionBridge`] test double.
//!
//! Before this, ~150 sites wrote `ToolRegistry::new()` + N `register(Arc::new(..))`
//! calls, and the only reusable permission double (`AllowAllPermissionBridge`)
//! lived inside one test binary. Message / conversation / compact builders
//! already live in this crate — these fill the registry / permission gap.

use std::collections::HashSet;
use std::sync::Arc;

use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolPermissionBridge;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolPermissionDecision;
use coco_tool_runtime::ToolPermissionRequest;
use coco_tool_runtime::ToolPermissionResolution;
use coco_tool_runtime::ToolRegistry;

/// Builds an [`Arc<ToolRegistry>`] from the real built-in tool impls. The tools
/// are zero-sized, so registration is cheap; execution (which pulls shell /
/// sandbox) only happens if a test actually invokes them.
#[derive(Default)]
pub struct ToolRegistryBuilder {
    registry: ToolRegistry,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bash(self) -> Self {
        self.registry.register(Arc::new(coco_tools::BashTool));
        self
    }

    pub fn with_read(self) -> Self {
        self.registry.register(Arc::new(coco_tools::ReadTool));
        self
    }

    pub fn with_write(self) -> Self {
        self.registry.register(Arc::new(coco_tools::WriteTool));
        self
    }

    pub fn with_edit(self) -> Self {
        self.registry.register(Arc::new(coco_tools::EditTool));
        self
    }

    pub fn with_glob(self) -> Self {
        self.registry.register(Arc::new(coco_tools::GlobTool));
        self
    }

    pub fn with_grep(self) -> Self {
        self.registry.register(Arc::new(coco_tools::GrepTool));
        self
    }

    /// Register all six core file/shell tools (Bash, Read, Write, Edit, Glob, Grep).
    pub fn with_core(self) -> Self {
        self.with_bash()
            .with_read()
            .with_write()
            .with_edit()
            .with_glob()
            .with_grep()
    }

    /// Register an arbitrary tool (e.g. a test-local fake or an MCP tool).
    pub fn register(self, tool: Arc<dyn DynTool>) -> Self {
        self.registry.register(tool);
        self
    }

    pub fn build(self) -> Arc<ToolRegistry> {
        Arc::new(self.registry)
    }
}

/// A [`ToolPermissionBridge`] double whose verdict is fixed at construction.
/// `allowed == None` approves everything; `Some(set)` approves a request iff its
/// `tool_name` is in the set, rejecting all others.
#[derive(Debug)]
struct ListPermissionBridge {
    allowed: Option<HashSet<String>>,
}

#[async_trait::async_trait]
impl ToolPermissionBridge for ListPermissionBridge {
    async fn request_permission(
        &self,
        request: ToolPermissionRequest,
    ) -> Result<ToolPermissionResolution, String> {
        let approved = match &self.allowed {
            None => true,
            Some(set) => set.contains(&request.tool_name),
        };
        Ok(ToolPermissionResolution {
            decision: if approved {
                ToolPermissionDecision::Approved
            } else {
                ToolPermissionDecision::Rejected
            },
            ..Default::default()
        })
    }
}

/// Builds a [`ToolPermissionBridgeRef`] test double. Use [`Self::allow_all`] /
/// [`Self::deny_all`] for the blanket cases, or chain [`Self::allow_tool`] to
/// approve a specific allow-list and reject everything else.
#[derive(Default)]
pub struct PermissionBridgeBuilder {
    allowed: HashSet<String>,
}

impl PermissionBridgeBuilder {
    /// Start with an empty allow-list (rejects every tool until one is added).
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool name (e.g. `"Bash"`) to the approve list.
    pub fn allow_tool(mut self, name: impl Into<String>) -> Self {
        self.allowed.insert(name.into());
        self
    }

    /// Build a bridge approving only the accumulated allow-list.
    pub fn build(self) -> ToolPermissionBridgeRef {
        Arc::new(ListPermissionBridge {
            allowed: Some(self.allowed),
        })
    }

    /// A bridge that approves every permission request.
    pub fn allow_all() -> ToolPermissionBridgeRef {
        Arc::new(ListPermissionBridge { allowed: None })
    }

    /// A bridge that rejects every permission request.
    pub fn deny_all() -> ToolPermissionBridgeRef {
        Arc::new(ListPermissionBridge {
            allowed: Some(HashSet::new()),
        })
    }
}

#[cfg(test)]
#[path = "registry.test.rs"]
mod tests;
