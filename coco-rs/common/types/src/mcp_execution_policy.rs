//! MCP execution trust policy.

use serde::Deserialize;
use serde::Serialize;

/// How much authority an MCP server's self-declared tool annotations receive.
///
/// MCP annotations are untrusted server input. The default therefore requires
/// approval for every call; trusting read-only hints or all calls must be an
/// explicit operator choice.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpExecutionPolicy {
    /// Require approval for every MCP tool call.
    #[default]
    AlwaysAsk,
    /// Auto-approve only tools the server marks as read-only.
    TrustReadOnlyHints,
    /// Auto-approve every tool call from the selected server.
    Full,
}

#[cfg(test)]
#[path = "mcp_execution_policy.test.rs"]
mod tests;
