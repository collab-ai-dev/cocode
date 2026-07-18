//! MCP tool exposure policy — how MCP tool schemas reach the model.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;

/// Policy value for how one MCP server's tools are exposed to the model.
///
/// Scoped to MCP tools; built-in deferred tools keep their own placement policy.
/// A runtime config provides a global default plus per-server overrides. The
/// effective placement also depends on the active model's discovery capability.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolExposure {
    /// Load every surviving MCP tool schema into the provider request.
    Load,
    /// Prefer provider-native or client-side schema discovery. If the model has
    /// no supported promotion strategy, fall back to the `use_tool` carrier
    /// rather than sending every schema eagerly.
    #[default]
    Defer,
    /// Never place MCP schemas in the model's direct tool list. Discover via
    /// ToolSearch (client JSON) and invoke through the `use_tool` carrier.
    UseTool,
}

impl McpToolExposure {
    /// Restrictiveness order for never-widen inheritance:
    /// `Load` (most exposed) > `Defer` > `UseTool` (least). A child may keep or
    /// reduce schema exposure, never increase it.
    fn restrictiveness(self) -> u8 {
        match self {
            Self::Load => 2,
            Self::Defer => 1,
            Self::UseTool => 0,
        }
    }

    /// Combine a parent policy with a child's requested policy, never widening:
    /// the result is the more restrictive of the two.
    pub fn restrict(parent: Self, requested: Self) -> Self {
        if requested.restrictiveness() <= parent.restrictiveness() {
            requested
        } else {
            parent
        }
    }

    /// Combine default + per-server policies across an inheritance boundary.
    /// The returned map contains only servers whose effective value differs
    /// from the restricted default.
    pub fn restrict_server_overrides(
        parent_default: Self,
        parent: &HashMap<String, Self>,
        requested_default: Self,
        requested: &HashMap<String, Self>,
    ) -> HashMap<String, Self> {
        let default = Self::restrict(parent_default, requested_default);
        let servers: HashSet<&String> = parent.keys().chain(requested.keys()).collect();
        servers
            .into_iter()
            .filter_map(|server| {
                let parent_value = parent.get(server).copied().unwrap_or(parent_default);
                let requested_value = requested.get(server).copied().unwrap_or(requested_default);
                let effective = Self::restrict(parent_value, requested_value);
                (effective != default).then(|| (server.clone(), effective))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseMcpToolExposureError;

impl std::fmt::Display for ParseMcpToolExposureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("expected load, defer, or use_tool")
    }
}

impl std::error::Error for ParseMcpToolExposureError {}

impl std::str::FromStr for McpToolExposure {
    type Err = ParseMcpToolExposureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "load" => Ok(Self::Load),
            "defer" => Ok(Self::Defer),
            "use_tool" => Ok(Self::UseTool),
            _ => Err(ParseMcpToolExposureError),
        }
    }
}

#[cfg(test)]
#[path = "mcp_exposure.test.rs"]
mod tests;
