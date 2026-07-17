//! Multi-source MCP config loading and deduplication.
//!
//! Sources are loaded in precedence order, a later source overriding an
//! earlier one by server name, so policy scopes (enterprise, managed) load
//! last and cannot be name-shadowed by user/project/local definitions
//!:
//! claudeai → user → project → local → enterprise → managed

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

/// Filesystem roots used while loading MCP config layers.
#[derive(Debug, Clone, Copy)]
pub struct McpConfigRoots<'a> {
    /// Project-scoped config root: `.mcp.json` and `.coco/mcp.json`.
    pub project_root: &'a Path,
    /// Session-local root: `.coco.local/mcp.json`.
    pub session_cwd: &'a Path,
}

/// A server entry as *written* in a config file, before the loader's
/// disabled/shape filtering drops it.
///
/// Deduplicated by name across [`config_paths`] exactly like the loader, so
/// `path`/`scope` name the file holding the *effective* definition.
#[derive(Debug, Clone)]
pub struct DefinedMcpServer {
    pub name: String,
    pub scope: ConfigScope,
    /// File holding the effective definition — the one an edit must target.
    pub path: PathBuf,
    /// `"disabled": true`. The loader skips these entirely, so a disabled entry
    /// never appears in [`McpConfigLoader::load_with_roots`] output.
    pub disabled: bool,
    /// The parsed shape, ignoring `disabled`. `None` = unrecognized entry
    /// (neither `command` nor `url`), which the loader also skips.
    pub config: Option<McpServerConfig>,
}

/// The MCP config files, in load order: a later entry overrides an earlier one
/// by server name.
///
/// Single source of truth for *which files are MCP config*. The loader and
/// every caller that locates a server's defining file iterate exactly this
/// list, so the two cannot drift apart.
pub fn config_paths(roots: McpConfigRoots<'_>, config_home: &Path) -> Vec<(PathBuf, ConfigScope)> {
    // Non-policy scopes come first (most-local last) so a later source overrides
    // an earlier one by server name (more-local-wins layering). Policy scopes
    // (enterprise, managed) come LAST so they win outright and cannot be
    // name-shadowed by user/project/local definitions.
    //
    // 1. Claude.ai scope: fetched at startup (not from file), so it has no entry
    //    here. Callers use `register_claudeai_configs()` to add these
    //    dynamically; it sits below user/project/local.
    let local_dir = format!("{}.local", coco_utils_common::COCO_CONFIG_DIR_NAME);
    vec![
        // 2. User scope — below project so a project definition wins a name
        //    collision.
        (config_home.join("mcp.json"), ConfigScope::User),
        // 3. Project scope: .mcp.json in project directory
        (roots.project_root.join(".mcp.json"), ConfigScope::Project),
        (
            roots
                .project_root
                .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
                .join("mcp.json"),
            ConfigScope::Project,
        ),
        // 4. Local scope
        (
            roots.session_cwd.join(local_dir).join("mcp.json"),
            ConfigScope::Local,
        ),
        // 5. Enterprise scope: enterprise-managed policy config. Loads after
        //    user/project/local so it wins a name collision and cannot be
        //    shadowed.
        (
            config_home.join("enterprise-mcp.json"),
            ConfigScope::Enterprise,
        ),
        // 6. Managed scope: policy-pushed config. Loads LAST so a managed
        //    definition wins outright over every other scope, enterprise
        //    included.
        (config_home.join("managed-mcp.json"), ConfigScope::Managed),
    ]
}

/// Every server *defined* across [`config_paths`], keyed to its effective
/// definition, including entries the loader skips (disabled or unparseable).
///
/// Matches on raw key presence rather than [`parse_server_config`], so a
/// `"disabled": true` entry — invisible to the loader — is still reported.
/// `/mcp enable` depends on that.
pub fn defined_servers(roots: McpConfigRoots<'_>, config_home: &Path) -> Vec<DefinedMcpServer> {
    let mut by_name: HashMap<String, DefinedMcpServer> = HashMap::new();
    for (path, scope) in config_paths(roots, config_home) {
        let Some(servers) = read_mcp_servers(&path) else {
            continue;
        };
        for (name, value) in servers {
            let disabled = value
                .get("disabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            // Parse against a copy without `disabled` so the transport is known
            // even for entries the loader skips.
            let mut enabled_value = value.clone();
            if let Some(obj) = enabled_value.as_object_mut() {
                obj.remove("disabled");
            }
            by_name.insert(
                name.clone(),
                DefinedMcpServer {
                    name,
                    scope,
                    path: path.clone(),
                    disabled,
                    config: parse_server_config(&enabled_value),
                },
            );
        }
    }
    by_name.into_values().collect()
}

/// Locate the file where `name` is effectively defined.
///
/// Writing `"disabled": true` into a *later*-loading file does not disable an
/// earlier definition (the loader simply skips the disabled entry and keeps the
/// earlier one), so an edit must target the defining file this returns.
pub fn defining_path(
    name: &str,
    roots: McpConfigRoots<'_>,
    config_home: &Path,
) -> Option<(PathBuf, ConfigScope)> {
    defined_servers(roots, config_home)
        .into_iter()
        .find(|server| server.name == name)
        .map(|server| (server.path, server.scope))
}

impl McpConfigLoader {
    /// Load MCP configs from all sources, deduplicating by server name.
    ///
    /// Later sources override earlier ones (by server name).
    pub fn load(cwd: &Path, config_home: &Path) -> Vec<ScopedMcpServerConfig> {
        Self::load_with_roots(
            McpConfigRoots {
                project_root: cwd,
                session_cwd: cwd,
            },
            config_home,
        )
    }

    /// Load MCP configs using distinct project and session roots.
    ///
    /// Project files are rooted at `project_root`; local files stay rooted at
    /// `session_cwd` so callers can split ProjectServices-owned config from
    /// session-local overrides without changing layer priority.
    pub fn load_with_roots(
        roots: McpConfigRoots<'_>,
        config_home: &Path,
    ) -> Vec<ScopedMcpServerConfig> {
        let mut configs_by_name: HashMap<String, ScopedMcpServerConfig> = HashMap::new();

        for (path, scope) in config_paths(roots, config_home) {
            load_mcp_json(&path, scope, &mut configs_by_name);
        }

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

    /// Resolve the config home directory.
    pub fn config_home() -> PathBuf {
        coco_config::global_config::config_home()
    }
}

/// Read the `mcpServers` object out of a config file, if it exists and parses.
fn read_mcp_servers(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    if !path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(?path, error = %e, "failed to read MCP config file");
            return None;
        }
    };
    let value = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!(?path, error = %e, "failed to parse MCP config JSON");
            return None;
        }
    };
    value.get("mcpServers").and_then(|s| s.as_object()).cloned()
}

/// Load servers from a single .mcp.json file, deduplicating by name.
fn load_mcp_json(
    path: &Path,
    scope: ConfigScope,
    configs: &mut HashMap<String, ScopedMcpServerConfig>,
) {
    let Some(servers) = read_mcp_servers(path) else {
        return;
    };

    for (name, server_config) in &servers {
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
