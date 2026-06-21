//! `/mcp` — MCP server management (list, add, remove, enable, disable).
//!
//! Reads MCP server configuration from `.coco/settings.json` and
//! `.mcp.json`, displays status for each server, and supports
//! enable/disable/add/remove subcommands.

use std::path::Path;
use std::pin::Pin;

/// A discovered MCP server entry from config files.
struct McpServer {
    name: String,
    command: String,
    args: Vec<String>,
    source: String,
    disabled: bool,
}

/// Async handler for `/mcp [list|add|remove|enable|disable]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let subcommand = args.trim().to_string();

        match subcommand.as_str() {
            "" | "list" => list_mcp_servers().await,
            _ => {
                if let Some(name) = subcommand.strip_prefix("enable ") {
                    toggle_server(name.trim(), /*enable*/ true).await
                } else if let Some(name) = subcommand.strip_prefix("disable ") {
                    toggle_server(name.trim(), /*enable*/ false).await
                } else if let Some(rest) = subcommand.strip_prefix("add ") {
                    add_server(rest.trim()).await
                } else if let Some(name) = subcommand.strip_prefix("remove ") {
                    remove_server(name.trim()).await
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
    })
}

/// List all MCP servers from config files.
async fn list_mcp_servers() -> crate::Result<String> {
    let mut servers = Vec::new();

    // Load from .coco/settings.json
    load_servers_from_file(
        Path::new(".coco/settings.json"),
        ".coco/settings.json",
        &mut servers,
    )
    .await;

    // Load from .mcp.json
    load_servers_from_file(Path::new(".mcp.json"), ".mcp.json", &mut servers).await;

    // Load from user-level config
    let user_settings = coco_config::global_config::user_settings_path();
    load_servers_from_file(&user_settings, "~/.coco/settings.json", &mut servers).await;

    let mut out = String::from("## MCP Servers\n\n");

    if servers.is_empty() {
        out.push_str("No MCP servers configured.\n\n");
        out.push_str("Configure servers in:\n");
        out.push_str("  .coco/settings.json  (project-level)\n");
        out.push_str("  .mcp.json              (project-level, shared)\n");
        out.push_str("  ~/.coco/settings.json (user-level)\n\n");
        out.push_str("Example config in .coco/settings.json:\n");
        out.push_str("  {\n");
        out.push_str("    \"mcpServers\": {\n");
        out.push_str("      \"my-server\": {\n");
        out.push_str("        \"command\": \"npx\",\n");
        out.push_str("        \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\"]\n");
        out.push_str("      }\n");
        out.push_str("    }\n");
        out.push_str("  }");
    } else {
        out.push_str(&format!(
            "{} server{} configured:\n\n",
            servers.len(),
            if servers.len() == 1 { "" } else { "s" }
        ));

        for server in &servers {
            let status = if server.disabled {
                "disabled"
            } else {
                "active"
            };
            let status_icon = if server.disabled { "[-]" } else { "[+]" };
            out.push_str(&format!(
                "  {status_icon} {:<20} {status:<10} ({})\n",
                server.name, server.source
            ));
            out.push_str(&format!("      cmd: {}", server.command));
            if !server.args.is_empty() {
                out.push_str(&format!(" {}", server.args.join(" ")));
            }
            out.push('\n');
        }
    }

    out.push_str("\n\nCommands:\n");
    out.push_str("  /mcp enable <name>     Enable a server\n");
    out.push_str("  /mcp disable <name>    Disable a server\n");
    out.push_str("  /mcp add <name> <cmd>  Add a new server\n");
    out.push_str("  /mcp remove <name>     Remove a server");

    Ok(out)
}

/// Load MCP servers from a JSON settings file.
async fn load_servers_from_file(path: &Path, source_label: &str, servers: &mut Vec<McpServer>) {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return;
    };

    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };

    let Some(mcp_servers) = parsed.get("mcpServers").and_then(|v| v.as_object()) else {
        return;
    };

    for (name, config) in mcp_servers {
        let command = config
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let args = config
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let disabled = config
            .get("disabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        servers.push(McpServer {
            name: name.clone(),
            command,
            args,
            source: source_label.to_string(),
            disabled,
        });
    }
}

/// Enable or disable a server by updating .coco/settings.json.
async fn toggle_server(name: &str, enable: bool) -> crate::Result<String> {
    let path = Path::new(".coco/settings.json");
    let action = if enable { "Enabling" } else { "Disabling" };

    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(format!(
            "Cannot {action} '{name}': .coco/settings.json not found.\n\
             Create the file with MCP server configuration first."
        ));
    };

    let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Ok("Cannot parse .coco/settings.json".to_string());
    };

    let servers = parsed.get_mut("mcpServers").and_then(|v| v.as_object_mut());

    let Some(servers) = servers else {
        return Ok("No MCP servers in .coco/settings.json".to_string());
    };

    if let Some(server_config) = servers.get_mut(name) {
        if enable {
            if let Some(obj) = server_config.as_object_mut() {
                obj.remove("disabled");
            }
        } else {
            server_config["disabled"] = serde_json::Value::Bool(true);
        }

        let new_content = serde_json::to_string_pretty(&parsed)?;
        tokio::fs::write(path, new_content).await?;

        Ok(format!("{action} MCP server: {name}"))
    } else {
        Ok(format!(
            "MCP server '{name}' not found in .coco/settings.json"
        ))
    }
}

/// Add a new MCP server to .coco/settings.json.
async fn add_server(input: &str) -> crate::Result<String> {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    if parts.len() < 2 {
        return Ok("Usage: /mcp add <name> <command> [args...]\n\n\
             Example: /mcp add my-server npx -y @modelcontextprotocol/server-filesystem"
            .to_string());
    }

    let name = parts[0];
    let cmd_parts: Vec<&str> = parts[1].split_whitespace().collect();
    let command = cmd_parts[0];
    let args: Vec<&str> = cmd_parts[1..].to_vec();

    let path = Path::new(".coco/settings.json");
    let mut parsed = if let Ok(content) = tokio::fs::read_to_string(path).await {
        serde_json::from_str::<serde_json::Value>(&content)
            .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        // Ensure .coco directory exists
        tokio::fs::create_dir_all(".coco").await?;
        serde_json::json!({})
    };

    let Some(root_obj) = parsed.as_object_mut() else {
        return Err(crate::CommandsError::generic(
            "settings.json root is not a JSON object",
        ));
    };
    let mcp_servers = root_obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let server_config = serde_json::json!({
        "command": command,
        "args": args,
    });

    mcp_servers[name] = server_config;

    let new_content = serde_json::to_string_pretty(&parsed)?;
    tokio::fs::write(path, new_content).await?;

    Ok(format!(
        "Added MCP server '{name}' to .coco/settings.json\n\
         Command: {command} {}\n\
         Restart the session to connect.",
        args.join(" "),
    ))
}

/// Remove an MCP server from .coco/settings.json.
async fn remove_server(name: &str) -> crate::Result<String> {
    let path = Path::new(".coco/settings.json");

    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok("Cannot remove: .coco/settings.json not found.".to_string());
    };

    let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Ok("Cannot parse .coco/settings.json".to_string());
    };

    let removed = parsed
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .and_then(|obj| obj.remove(name));

    match removed {
        Some(_) => {
            let new_content = serde_json::to_string_pretty(&parsed)?;
            tokio::fs::write(path, new_content).await?;
            Ok(format!("Removed MCP server: {name}"))
        }
        None => Ok(format!(
            "MCP server '{name}' not found in .coco/settings.json"
        )),
    }
}

#[cfg(test)]
#[path = "mcp.test.rs"]
mod tests;
