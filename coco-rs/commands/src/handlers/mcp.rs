//! `/mcp` — MCP server management (list, add, remove, enable, disable).
//!
//! Reads and writes the same files as [`coco_mcp::McpConfigLoader`], via
//! [`coco_mcp::config_paths`], so what `/mcp list` shows is what actually
//! loads and what `/mcp add|enable|disable|remove` writes actually takes
//! effect. Settings.json is deliberately not consulted: the loader never reads
//! it, and `mcpServers` is not a Settings field.

use std::path::Path;
use std::path::PathBuf;

use coco_mcp::ConfigScope;
use coco_mcp::McpConfigLoader;
use coco_mcp::McpConfigRoots;
use coco_mcp::McpServerConfig;

/// Filesystem roots the `/mcp` handler resolves config against.
struct McpPaths {
    project_root: PathBuf,
    session_cwd: PathBuf,
    config_home: PathBuf,
}

impl McpPaths {
    /// Resolve against the session cwd threaded in from registration, mirroring
    /// the loader's `load(cwd, ..)`.
    fn new(cwd: PathBuf) -> Self {
        Self {
            project_root: cwd.clone(),
            session_cwd: cwd,
            config_home: McpConfigLoader::config_home(),
        }
    }

    fn roots(&self) -> McpConfigRoots<'_> {
        McpConfigRoots {
            project_root: &self.project_root,
            session_cwd: &self.session_cwd,
        }
    }

    /// Where `/mcp add` writes: the project-scoped `<root>/.cocode/mcp.json`.
    fn add_target(&self) -> PathBuf {
        self.project_root
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("mcp.json")
    }
}

/// Run `/mcp [list|add|remove|enable|disable]` against `cwd`.
///
/// `cwd` is the session cwd captured at registration, never the process cwd: a
/// single app-server process hosts sessions from different projects, so reading
/// the process cwd here would operate on the wrong project (§6.5, D-37).
pub async fn run(args: &str, cwd: &Path) -> crate::Result<String> {
    let subcommand = args.trim();
    let paths = McpPaths::new(cwd.to_path_buf());

    match subcommand {
        "" | "list" => list_mcp_servers(&paths).await,
        _ => {
            if let Some(name) = subcommand.strip_prefix("enable ") {
                toggle_server(name.trim(), /*enable*/ true, &paths).await
            } else if let Some(name) = subcommand.strip_prefix("disable ") {
                toggle_server(name.trim(), /*enable*/ false, &paths).await
            } else if let Some(rest) = subcommand.strip_prefix("add ") {
                add_server(rest.trim(), &paths).await
            } else if let Some(name) = subcommand.strip_prefix("remove ") {
                remove_server(name.trim(), &paths).await
            } else {
                Ok(format!(
                    "Unknown MCP subcommand: {subcommand}\n\n\
                     Usage:\n\
                     /mcp              List configured MCP servers\n\
                     /mcp enable <n>   Enable a disabled server\n\
                     /mcp disable <n>  Disable a server\n\
                     /mcp add <n> <cmd> [args...]  Add a new server\n\
                     /mcp remove <n>   Remove a server"
                ))
            }
        }
    }
}

/// List MCP servers exactly as the loader sees them.
async fn list_mcp_servers(paths: &McpPaths) -> crate::Result<String> {
    // The loader is the authority on what actually loads; `defined_servers`
    // adds the source file plus the entries the loader skips (disabled or
    // malformed), which would otherwise be invisible here.
    let loaded = McpConfigLoader::load_with_roots(paths.roots(), &paths.config_home);
    let mut defined = coco_mcp::defined_servers(paths.roots(), &paths.config_home);
    defined.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::from("## MCP Servers\n\n");

    if defined.is_empty() {
        out.push_str(&empty_state(paths));
    } else {
        out.push_str(&format!(
            "{} server{} configured:\n\n",
            defined.len(),
            if defined.len() == 1 { "" } else { "s" }
        ));

        for server in &defined {
            let is_loaded = loaded.iter().any(|s| s.name == server.name);
            let (status_icon, status) = match (server.disabled, is_loaded) {
                (true, _) => ("[-]", "disabled"),
                (false, true) => ("[+]", "active"),
                // Neither disabled nor loaded: the entry has no `command` and
                // no `url`, so `parse_server_config` rejects it.
                (false, false) => ("[!]", "invalid"),
            };
            let transport = server.config.as_ref().map_or("-", transport_label);
            out.push_str(&format!(
                "  {status_icon} {:<20} {status:<9} {transport:<6} {}\n",
                server.name,
                scope_label(server.scope),
            ));
            out.push_str(&format!("      {}\n", display_path(&server.path)));
        }
    }

    out.push_str("\n\nCommands:\n");
    out.push_str("  /mcp enable <name>     Enable a server\n");
    out.push_str("  /mcp disable <name>    Disable a server\n");
    out.push_str("  /mcp add <name> <cmd>  Add a new server\n");
    out.push_str("  /mcp remove <name>     Remove a server");

    Ok(out)
}

/// Help text naming the files the loader actually reads.
fn empty_state(paths: &McpPaths) -> String {
    let dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
    let mut out = String::from("No MCP servers configured.\n\n");
    out.push_str("Configure servers in:\n");
    out.push_str(&format!("  ~/{dir}/mcp.json     (user-level)\n"));
    out.push_str("  .mcp.json            (project-level, shared)\n");
    out.push_str(&format!("  {dir}/mcp.json     (project-level)\n\n"));
    out.push_str(&format!(
        "Example config in {}:\n",
        display_path(&paths.add_target())
    ));
    out.push_str("  {\n");
    out.push_str("    \"mcpServers\": {\n");
    out.push_str("      \"my-server\": {\n");
    out.push_str("        \"command\": \"npx\",\n");
    out.push_str("        \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\"]\n");
    out.push_str("      }\n");
    out.push_str("    }\n");
    out.push_str("  }");
    out
}

/// Enable or disable a server by editing the file that defines it.
async fn toggle_server(name: &str, enable: bool, paths: &McpPaths) -> crate::Result<String> {
    let action = if enable { "Enabled" } else { "Disabled" };

    let Some((path, scope)) = coco_mcp::defining_path(name, paths.roots(), &paths.config_home)
    else {
        return Ok(not_found_message(name, paths));
    };
    if let Some(refusal) = refuse_policy_scope(name, scope, &path) {
        return Ok(refusal);
    }

    let mut parsed = read_json(&path).await?;
    let Some(server_config) = parsed
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .and_then(|servers| servers.get_mut(name))
    else {
        return Ok(format!(
            "MCP server '{name}' not found in {}",
            display_path(&path)
        ));
    };

    if enable {
        if let Some(obj) = server_config.as_object_mut() {
            obj.remove("disabled");
        }
    } else {
        server_config["disabled"] = serde_json::Value::Bool(true);
    }
    write_json(&path, &parsed).await?;

    let mut out = format!("{action} MCP server '{name}' in {}", display_path(&path));
    // A lower-precedence file may define the same name. The loader skips the
    // now-disabled entry and falls back to that one, so say so rather than
    // report a disable that didn't take.
    match (enable, residual_note(name, paths)) {
        (false, Some(note)) => out.push_str(&note),
        (false, None) | (true, _) => out.push_str("\nRestart the session to apply."),
    }
    Ok(out)
}

/// Add a server to the project-scoped `.cocode/mcp.json`.
async fn add_server(input: &str, paths: &McpPaths) -> crate::Result<String> {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let [name, rest] = parts[..] else {
        return Ok("Usage: /mcp add <name> <command> [args...]\n\n\
             Example: /mcp add my-server npx -y @modelcontextprotocol/server-filesystem"
            .to_string());
    };
    let cmd_parts: Vec<&str> = rest.split_whitespace().collect();
    let Some((command, args)) = cmd_parts.split_first() else {
        return Ok("Usage: /mcp add <name> <command> [args...]\n\n\
             Example: /mcp add my-server npx -y @modelcontextprotocol/server-filesystem"
            .to_string());
    };

    let path = paths.add_target();
    let mut parsed = read_json(&path).await?;
    if parsed.is_null() {
        parsed = serde_json::json!({});
    }

    let Some(root_obj) = parsed.as_object_mut() else {
        return Err(crate::CommandsError::generic(format!(
            "{} root is not a JSON object",
            display_path(&path)
        )));
    };
    let mcp_servers = root_obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    mcp_servers[name] = serde_json::json!({
        "command": command,
        "args": args,
    });

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    write_json(&path, &parsed).await?;

    let mut out = format!(
        "Added MCP server '{name}' to {}\nCommand: {command} {}",
        display_path(&path),
        args.join(" ")
    );
    // Anything loading after .cocode/mcp.json shadows this entry by name.
    if let Some((shadow_path, shadow_scope)) = shadowing_definition(name, &path, paths) {
        out.push_str(&format!(
            "\n\nNote: '{name}' is also defined in {} ({}), which loads later and wins. \
             The new entry will not take effect until that one is removed.",
            display_path(&shadow_path),
            scope_label(shadow_scope)
        ));
    } else {
        out.push_str("\nRestart the session to connect.");
    }
    Ok(out)
}

/// Remove a server from the file that defines it.
async fn remove_server(name: &str, paths: &McpPaths) -> crate::Result<String> {
    let Some((path, scope)) = coco_mcp::defining_path(name, paths.roots(), &paths.config_home)
    else {
        return Ok(not_found_message(name, paths));
    };
    if let Some(refusal) = refuse_policy_scope(name, scope, &path) {
        return Ok(refusal);
    }

    let mut parsed = read_json(&path).await?;
    let removed = parsed
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .and_then(|obj| obj.remove(name));

    if removed.is_none() {
        return Ok(format!(
            "MCP server '{name}' not found in {}",
            display_path(&path)
        ));
    }
    write_json(&path, &parsed).await?;

    let mut out = format!("Removed MCP server '{name}' from {}", display_path(&path));
    if let Some(note) = residual_note(name, paths) {
        out.push_str(&note);
    } else {
        out.push_str("\nRestart the session to apply.");
    }
    Ok(out)
}

/// Report a definition that survives an edit meant to stop the server loading.
///
/// Asks the loader, so it only fires when a *lower*-precedence file defines the
/// same name: the loader drops the edited entry and silently falls back to that
/// definition, and the disable/remove the user just ran doesn't take.
fn residual_note(name: &str, paths: &McpPaths) -> Option<String> {
    let survivor = McpConfigLoader::load_with_roots(paths.roots(), &paths.config_home)
        .into_iter()
        .find(|server| server.name == name)?;
    Some(format!(
        "\n\nNote: '{name}' is also defined at {} scope and will keep loading from there.",
        scope_label(survivor.scope)
    ))
}

/// A definition of `name` in a file that loads *after* `target`, shadowing it.
fn shadowing_definition(
    name: &str,
    target: &Path,
    paths: &McpPaths,
) -> Option<(PathBuf, ConfigScope)> {
    let (defining, scope) = coco_mcp::defining_path(name, paths.roots(), &paths.config_home)?;
    (defining != target).then_some((defining, scope))
}

/// Refuse to edit enterprise/managed policy files.
fn refuse_policy_scope(name: &str, scope: ConfigScope, path: &Path) -> Option<String> {
    match scope {
        ConfigScope::Enterprise | ConfigScope::Managed => Some(format!(
            "Cannot modify '{name}': it is defined by {} policy in {}.\n\
             Policy-managed servers are not editable from /mcp — contact your administrator.",
            scope_label(scope),
            display_path(path)
        )),
        ConfigScope::Local
        | ConfigScope::User
        | ConfigScope::Project
        | ConfigScope::Dynamic
        | ConfigScope::ClaudeAi => None,
    }
}

fn not_found_message(name: &str, paths: &McpPaths) -> String {
    let searched = coco_mcp::config_paths(paths.roots(), &paths.config_home)
        .into_iter()
        .map(|(path, _)| format!("  {}", display_path(&path)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("MCP server '{name}' not found. Searched:\n{searched}")
}

async fn read_json(path: &Path) -> crate::Result<serde_json::Value> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(serde_json::json!({}));
    };
    if content.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    Ok(serde_json::from_str(&content)?)
}

async fn write_json(path: &Path, value: &serde_json::Value) -> crate::Result<()> {
    let mut content = serde_json::to_string_pretty(value)?;
    content.push('\n');
    tokio::fs::write(path, content).await?;
    Ok(())
}

fn transport_label(config: &McpServerConfig) -> &'static str {
    match config {
        McpServerConfig::Stdio(_) => "stdio",
        McpServerConfig::Sse(_) => "sse",
        McpServerConfig::Http(_) => "http",
        McpServerConfig::WebSocket(_) => "ws",
        McpServerConfig::ClientHosted(_) => "client",
        McpServerConfig::ClaudeAiProxy(_) => "proxy",
    }
}

fn scope_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "local",
        ConfigScope::User => "user",
        ConfigScope::Project => "project",
        ConfigScope::Dynamic => "dynamic",
        ConfigScope::Enterprise => "enterprise",
        ConfigScope::ClaudeAi => "claude.ai",
        ConfigScope::Managed => "managed",
    }
}

/// Render a path for display, shortening the home prefix to `~`.
fn display_path(path: &Path) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
#[path = "mcp.test.rs"]
mod tests;
