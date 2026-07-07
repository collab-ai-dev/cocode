//! Async services driven by the agent loop.
//!
//! - [`extract`] — turn-end memory extraction (forked subagent)
//! - [`dream`] — periodic auto-dream consolidation (forked subagent)
//! - [`session`] — per-session 9-section markdown insights

use std::sync::Arc;

use arc_swap::ArcSwap;
use coco_types::ActiveShellTool;
use coco_types::SessionId;
use coco_types::ToolOverrides;

pub mod dream;
pub mod extract;
pub mod session;

pub use dream::DreamService;
pub use extract::ExtractService;
pub use session::SessionMemoryService;

pub(crate) type SessionIdSlot = Arc<ArcSwap<SessionId>>;

/// Runtime-only tool selection shared by memory fork services.
#[derive(Clone)]
pub struct MemoryForkToolConfig {
    pub active_shell_tool: ActiveShellTool,
    pub tool_overrides: Arc<ToolOverrides>,
}

impl MemoryForkToolConfig {
    pub fn new(active_shell_tool: ActiveShellTool, tool_overrides: Arc<ToolOverrides>) -> Self {
        Self {
            active_shell_tool,
            tool_overrides,
        }
    }

    pub fn disabled() -> Self {
        Self::new(ActiveShellTool::Disabled, Arc::new(ToolOverrides::none()))
    }
}

#[cfg(test)]
pub(super) mod test_support;
