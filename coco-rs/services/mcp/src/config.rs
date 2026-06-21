//! Multi-source MCP config loading and deduplication.
//!
//! Sources checked in order (later overrides earlier by server name):
//! managed → enterprise → claudeai → project → user → local → dynamic

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use tracing::debug;
use tracing::warn;

use crate::types::ConfigScope;
use crate::types::McpHttpConfig;
use crate::types::McpOAuthConfig;
use crate::types::McpServerConfig;
use crate::types::McpSseConfig;
use crate::types::McpStdioConfig;
use crate::types::ScopedMcpServerConfig;

/// Loads MCP server configurations from all sources.
pub struct McpConfigLoader;

impl McpConfigLoader {
    /// Load MCP configs from all sources, deduplicating by server name.
    ///
    /// Later sources override earlier ones (by server name).
    pub fn load(cwd: &Path, config_home: &Path) -> Vec<ScopedMcpServerConfig> {
        let mut configs_by_name: HashMap<String, ScopedMcpServerConfig> = HashMap::new();

        // 1. Managed scope: policy-pushed config
        let managed_path = config_home.join("managed-mcp.json");
        load_mcp_json(&managed_path, ConfigScope::Managed, &mut configs_by_name);

        // 2. Enterprise scope: enterprise-managed config
        let enterprise_path = config_home.join("enterprise-mcp.json");
        load_mcp_json(
            &enterprise_path,
            ConfigScope::Enterprise,
            &mut configs_by_name,
        );

        // 3. Claude.ai scope: fetched at startup (not from file, loaded via register)
        //    Callers use `register_claudeai_configs()` to add these dynamically.

        // 4. Project scope: .mcp.json in project directory
        load_mcp_json(
            &cwd.join(".mcp.json"),
            ConfigScope::Project,
            &mut configs_by_name,
        );

        // 5. Project scope: .coco/mcp.json
        load_mcp_json(
            &cwd.join(".coco/mcp.json"),
            ConfigScope::Project,
            &mut configs_by_name,
        );

        // 6. User scope: ~/.coco/mcp.json (config_home)
        let user_mcp = config_home.join("mcp.json");
        load_mcp_json(&user_mcp, ConfigScope::User, &mut configs_by_name);

        // 7. Local scope: .coco.local/mcp.json (gitignored per-project)
        load_mcp_json(
            &cwd.join(".coco.local/mcp.json"),
            ConfigScope::Local,
            &mut configs_by_name,
        );

        configs_by_name.into_values().collect()
    }

    /// Register Claude.ai org-managed configs (fetched via API at startup).
    pub fn register_claudeai_configs(
        configs: &[ScopedMcpServerConfig],
        target: &mut Vec<ScopedMcpServerConfig>,
    ) {
        for config in configs {
            debug!(server = %config.name, "registering claude.ai MCP server");
            target.push(config.clone());
        }
    }

    /// Register dynamic (runtime) configs from plugins.
    pub fn register_dynamic_configs(
        configs: &[ScopedMcpServerConfig],
        target: &mut Vec<ScopedMcpServerConfig>,
    ) {
        for config in configs {
            debug!(server = %config.name, "registering dynamic MCP server");
            target.push(config.clone());
        }
    }

    /// Resolve the config home directory (~/.coco/ or `COCO_CONFIG_DIR`).
    ///
    /// Delegates to `coco_config::global_config::config_home` so MCP and
    /// the rest of the config surface always agree on the same path.
    pub fn config_home() -> PathBuf {
        coco_config::global_config::config_home()
    }
}

/// Load servers from a single .mcp.json file, deduplicating by name.
fn load_mcp_json(
    path: &Path,
    scope: ConfigScope,
    configs: &mut HashMap<String, ScopedMcpServerConfig>,
) {
    if !path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(?path, error = %e, "failed to read MCP config file");
            return;
        }
    };
    let value = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!(?path, error = %e, "failed to parse MCP config JSON");
            return;
        }
    };
    let Some(servers) = value.get("mcpServers").and_then(|s| s.as_object()) else {
        return;
    };

    for (name, server_config) in servers {
        if let Some(mut config) = parse_server_config(server_config) {
            // Expand ${VAR} / ${VAR:-default} references against the process
            // environment before the config reaches the launch/transport layer.
            let missing =
                crate::env_expansion::expand_config(&mut config, &crate::env_expansion::ProcessEnv);
            if !missing.is_empty() {
                warn!(
                    server = %name,
                    ?scope,
                    missing_vars = ?missing,
                    "MCP config references unset environment variables; left as literal ${{...}}"
                );
            }
            debug!(server = %name, ?scope, "loaded MCP server config");
            configs.insert(
                name.clone(),
                ScopedMcpServerConfig {
                    name: name.clone(),
                    config,
                    scope,
                    plugin_source: None,
                },
            );
        }
    }
}

/// Parse a server config from JSON. Detects the transport from the shape
/// (`command` → stdio, `url` → http/sse), so callers (settings + plugins)
/// don't need an explicit `transport` tag. Returns `None` for a disabled or
/// unrecognized entry.
pub fn parse_server_config(value: &serde_json::Value) -> Option<McpServerConfig> {
    // Check for disabled server
    if value
        .get("disabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    // Detect transport type
    if value.get("command").is_some() {
        return parse_stdio_config(value);
    }

    if let Some(url) = value.get("url").and_then(|u| u.as_str()) {
        let headers = parse_headers(value);
        let headers_helper = parse_headers_helper(value);
        let oauth = parse_oauth(value);
        let transport_type = value
            .get("transport")
            .and_then(|t| t.as_str())
            .unwrap_or("sse");

        return match transport_type {
            "http" => Some(McpServerConfig::Http(McpHttpConfig {
                url: url.to_string(),
                headers,
                headers_helper,
                oauth,
            })),
            _ => Some(McpServerConfig::Sse(McpSseConfig {
                url: url.to_string(),
                headers,
                headers_helper,
                oauth,
            })),
        };
    }

    None
}

fn parse_stdio_config(value: &serde_json::Value) -> Option<McpServerConfig> {
    let command = value.get("command")?.as_str()?.to_string();
    let args: Vec<String> = value
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env = parse_string_map(value, "env");
    let cwd = value.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from);

    Some(McpServerConfig::Stdio(McpStdioConfig {
        command,
        args,
        env,
        cwd,
    }))
}

fn parse_headers(value: &serde_json::Value) -> HashMap<String, String> {
    parse_string_map(value, "headers")
}

fn parse_headers_helper(value: &serde_json::Value) -> Option<String> {
    value
        .get("headersHelper")
        .or_else(|| value.get("headers_helper"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn parse_oauth(value: &serde_json::Value) -> Option<McpOAuthConfig> {
    let oauth = value.get("oauth")?;
    match serde_json::from_value(oauth.clone()) {
        Ok(config) => Some(config),
        Err(error) => {
            warn!(error = %error, "failed to parse MCP OAuth config");
            None
        }
    }
}

fn parse_string_map(value: &serde_json::Value, key: &str) -> HashMap<String, String> {
    value
        .get(key)
        .and_then(|e| e.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "config.test.rs"]
mod tests;
