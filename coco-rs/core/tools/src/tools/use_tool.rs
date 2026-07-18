//! The `use_tool` carrier.
//!
//! In `use_tool` MCP-exposure mode the model never sees MCP tool schemas in its
//! direct tool list. It discovers a tool via `ToolSearch` and invokes it
//! through this source-neutral carrier: `use_tool { name, arguments }`.
//!
//! [`UseToolTool`] is only a **schema carrier**. The query preparer
//! (`app/query::tool_call_preparer`) unwraps a `use_tool` call to its real
//! target BEFORE validation, permissions, hooks, concurrency classification,
//! and execution — keyed on the target's semantic identity while the provider
//! wire call keeps the `use_tool` name. This tool's own [`Tool::execute`] must
//! therefore never run; if it does, the resolver was bypassed and we fail
//! closed rather than silently no-op.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

const USE_TOOL_PROMPT: &str = "Invoke a tool that was discovered via ToolSearch but is not in your direct tool list. Pass the target tool's exact `name` (as returned by ToolSearch) and its `arguments` object. The call is routed to the real tool, which validates the arguments and runs under its own permissions. Use this only for names ToolSearch identified as use_tool targets; tools already in your tool list are called directly by name.";

/// Typed input for [`UseToolTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UseToolInput {
    /// Exact wire name of the target tool, as returned by ToolSearch.
    pub name: String,
    /// Arguments object forwarded verbatim to the target tool.
    #[serde(default)]
    pub arguments: Value,
}

pub struct UseToolTool;

#[async_trait::async_trait]
impl Tool for UseToolTool {
    type Input = UseToolInput;
    coco_tool_runtime::impl_runtime_schema!(UseToolInput);
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::UseTool)
    }
    fn name(&self) -> &str {
        ToolName::UseTool.as_str()
    }
    /// Present in the model's tool list whenever an MCP server needs the
    /// `use_tool` exposure path. The carrier schema is static — its presence
    /// never depends on which MCP servers are connected, keeping the definition
    /// byte-stable for prompt caching.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.use_tool_active()
    }
    /// A schema carrier is always loaded when needed, never deferred.
    fn should_defer(&self) -> bool {
        false
    }
    fn description(&self, _input: &UseToolInput, _options: &DescriptionOptions) -> String {
        USE_TOOL_PROMPT.into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        USE_TOOL_PROMPT.into()
    }
    fn render_for_model(&self, out: &Value) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: serde_json::to_string(out).unwrap_or_default(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        _input: UseToolInput,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        Err(ToolError::ExecutionFailed {
            message: "use_tool carrier reached execution without being unwrapped to its target"
                .into(),
            display_data: None,
            source: None,
        })
    }
}

#[cfg(test)]
#[path = "use_tool.test.rs"]
mod tests;
