//! `/mcp` — MCP server management (list, add, remove, enable, disable).
//!
//! Definitions ("what is this server") live in the mcp.json family, read and
//! written via [`coco_mcp::config_paths`]. Whether a defined server *runs* is
//! a separate concern with separate ownership: `/mcp enable|disable` writes
//! per-project user toggles into `GlobalConfig` (outside the repository), and
//! [`coco_mcp::McpActivationPolicy`] — the same authority the session loader
//! uses — resolves each server's activation for `/mcp list`. The two views
//! cannot disagree because there is only one.

use std::path::Path;
use std::path::PathBuf;

use coco_config::McpPolicyConfig;
use coco_config::global_config;
use coco_mcp::ConfigScope;
use coco_mcp::McpActivation;
use coco_mcp::McpActivationPolicy;
use coco_mcp::McpConfigLoader;
use coco_mcp::McpConfigRoots;
use coco_mcp::McpServerConfig;

/// The per-session inputs `/mcp` needs, captured at registry build time.
///
/// `project_root` is the *resolved* project root (the `ProjectServices` cache
/// key), not the session cwd — it anchors the project-scope config files and,
/// critically, the `GlobalConfig.projects` key the user toggles live under.
/// Using the cwd here would write toggles under a subdirectory key the session
/// loader never reads.
#[derive(Debug, Clone)]
pub struct McpCommandContext {
    pub project_root: PathBuf,
    pub session_cwd: PathBuf,
    /// Settings-side activation policy (trust gate + deny list), resolved at
    /// the `build_runtime_config` merge site.
    pub policy: McpPolicyConfig,
}

impl McpCommandContext {
    /// Both roots at `cwd`, default policy — for callers without a resolved
    /// project root (tests, the bare `register_extended_builtins`).
    pub fn for_cwd(cwd: PathBuf) -> Self {
        Self {
            project_root: cwd.clone(),
            session_cwd: cwd,
            policy: McpPolicyConfig::default(),
        }
    }
}

/// Filesystem roots the `/mcp` handler resolves config against.
struct McpPaths {
    project_root: PathBuf,
    session_cwd: PathBuf,
    config_home: PathBuf,
    /// User-side toggle store (`GlobalConfig`). Explicit so tests never touch
    /// the real per-user file.
    global_config: PathBuf,
}

impl McpPaths {
    /// Mirror the session loader's roots exactly: project files at the
    /// resolved project root, local files at the session cwd.
    fn new(context: &McpCommandContext) -> Self {
        Self {
            project_root: context.project_root.clone(),
            session_cwd: context.session_cwd.clone(),
            config_home: McpConfigLoader::config_home(),
            global_config: global_config::global_config_path(),
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

    /// The activation authority for this project — settings-side policy plus
    /// the current user toggles, re-read per invocation so an
    /// enable → list round trip observes the toggle immediately.
    fn activation_policy(&self, policy: &McpPolicyConfig) -> McpActivationPolicy {
        McpActivationPolicy::resolve_at(&self.global_config, &self.project_root, policy)
    }
}

/// Run `/mcp [list|add|remove|enable|disable]`.
///
/// The context roots are captured at registration, never read from the
/// process cwd: a single app-server process hosts sessions from different
/// projects, so reading the process cwd here would operate on the wrong
/// project (§6.5, D-37).
pub async fn run(args: &str, context: &McpCommandContext) -> crate::Result<String> {
    let subcommand = args.trim();
    let paths = McpPaths::new(context);
    let policy = &context.policy;

    match subcommand {
        "" | "list" => list_mcp_servers(&paths, policy).await,
        _ => {
            if let Some(name) = subcommand.strip_prefix("enable ") {
                enable_server(name.trim(), &paths, policy).await
            } else if let Some(name) = subcommand.strip_prefix("disable ") {
                disable_server(name.trim(), &paths).await
            } else if let Some(rest) = subcommand.strip_prefix("add ") {
                add_server(rest.trim(), &paths).await
            } else if let Some(name) = subcommand.strip_prefix("remove ") {
                remove_server(name.trim(), &paths).await
            } else {
                Ok(format!(
                    "Unknown MCP subcommand: {subcommand}\n\n\
                     Usage:\n\
                     /mcp              List configured MCP servers\n\
                     /mcp enable <n>   Enable (and approve) a server for this project\n\
                     /mcp disable <n>  Disable a server for this project\n\
                     /mcp add <n> <cmd> [args...]  Add a new server\n\
                     /mcp remove <n>   Remove a server definition"
                ))
            }
        }
    }
}

/// List MCP servers with the activation each one resolves to.
async fn list_mcp_servers(paths: &McpPaths, policy: &McpPolicyConfig) -> crate::Result<String> {
    let activation_policy = paths.activation_policy(policy);
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

        let mut awaiting = false;
        let mut legacy = false;
        for server in &defined {
            let (status_icon, status) = if server.config.is_none() && !server.legacy_disabled {
                // No `command` and no `url`: nothing could ever connect.
                ("[!]", "invalid")
            } else {
                match activation_policy.for_defined(server) {
                    McpActivation::Active => ("[+]", "active"),
                    McpActivation::UserDisabled => ("[-]", "disabled"),
                    McpActivation::AwaitingApproval => {
                        awaiting = true;
                        ("[?]", "needs approval")
                    }
                    McpActivation::PolicyDenied => ("[x]", "denied"),
                    McpActivation::LegacyDisabled => {
                        legacy = true;
                        ("[-]", "disabled*")
                    }
                }
            };
            let transport = server.config.as_ref().map_or("-", transport_label);
            out.push_str(&format!(
                "  {status_icon} {:<20} {status:<14} {transport:<6} {}\n",
                server.name,
                scope_label(server.scope),
            ));
            out.push_str(&format!("      {}\n", display_path(&server.path)));
        }
        if awaiting {
            out.push_str(
                "\nServers marked [?] come from this repository and won't connect \
                 until you approve them with /mcp enable <name>.\n",
            );
        }
        if legacy {
            out.push_str(
                "\ndisabled*: the entry still uses the removed \"disabled\" field \
                 and stays off. /mcp enable <name> migrates it.\n",
            );
        }
    }

    out.push_str("\n\nCommands:\n");
    out.push_str("  /mcp enable <name>     Enable (and approve) a server\n");
    out.push_str("  /mcp disable <name>    Disable a server for this project\n");
    out.push_str("  /mcp add <name> <cmd>  Add a new server\n");
    out.push_str("  /mcp remove <name>     Remove a server definition");

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

/// Enable a server: clear the user's disable toggle, record approval for a
/// repo-defined server, and migrate a residual legacy `"disabled"` field out
/// of the defining file. All state changes are user-side (GlobalConfig)
/// except the explicit legacy migration.
async fn enable_server(
    name: &str,
    paths: &McpPaths,
    policy: &McpPolicyConfig,
) -> crate::Result<String> {
    let Some(server) = find_defined(name, paths) else {
        return Ok(not_found_message(name, paths));
    };

    // A denied server can never activate; say so instead of writing a toggle
    // that has no effect.
    let activation_policy = paths.activation_policy(policy);
    if activation_policy.is_denied(name, server.config.as_ref()) {
        return Ok(format!(
            "Cannot enable '{name}': it is denied by a `denied_mcp_servers` settings entry.\n\
             Remove the deny entry (or ask your administrator) first."
        ));
    }

    // Migrate a residual legacy field: without this the fail-safe keeps the
    // server off no matter what the toggle says.
    if server.legacy_disabled {
        if let Some(refusal) = refuse_policy_scope(name, server.scope, &server.path) {
            return Ok(refusal);
        }
        strip_legacy_disabled(name, &server.path).await?;
    }

    let mut global = load_global(paths)?;
    let project = global
        .projects
        .entry(coco_mcp::project_key(&paths.project_root))
        .or_default();
    project.disabled_mcp_servers.remove(name);
    let newly_approved = server.scope == ConfigScope::Project
        && project.approved_mcp_servers.insert(name.to_string());
    global_config::write_global_config_at(&paths.global_config, &global)
        .map_err(|e| crate::CommandsError::generic(e.to_string()))?;

    let mut out = format!("Enabled MCP server '{name}' for this project");
    if newly_approved {
        out.push_str(" (repo-defined server approved)");
    }
    if server.legacy_disabled {
        out.push_str(&format!(
            "\nRemoved the legacy \"disabled\" field from {}.",
            display_path(&server.path)
        ));
    }
    out.push_str("\nRestart the session to apply.");
    Ok(out)
}

/// Disable a server for this project — a user-side toggle keyed by name, so
/// it holds regardless of which file defines the server (no lower-precedence
/// definition can sneak back in, unlike the old file-edit disable).
async fn disable_server(name: &str, paths: &McpPaths) -> crate::Result<String> {
    let Some(server) = find_defined(name, paths) else {
        return Ok(not_found_message(name, paths));
    };
    // Enterprise/managed servers are admin-mandated; the user toggle does not
    // override policy in either direction.
    if let Some(refusal) = refuse_policy_scope(name, server.scope, &server.path) {
        return Ok(refusal);
    }

    let mut global = load_global(paths)?;
    global
        .projects
        .entry(coco_mcp::project_key(&paths.project_root))
        .or_default()
        .disabled_mcp_servers
        .insert(name.to_string());
    global_config::write_global_config_at(&paths.global_config, &global)
        .map_err(|e| crate::CommandsError::generic(e.to_string()))?;

    Ok(format!(
        "Disabled MCP server '{name}' for this project\nRestart the session to apply."
    ))
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

/// The merged definition for `name`, if any.
fn find_defined(name: &str, paths: &McpPaths) -> Option<coco_mcp::DefinedMcpServer> {
    coco_mcp::defined_servers(paths.roots(), &paths.config_home)
        .into_iter()
        .find(|server| server.name == name)
}

fn load_global(paths: &McpPaths) -> crate::Result<coco_config::global_config::GlobalConfig> {
    global_config::load_global_config_at(&paths.global_config)
        .map_err(|e| crate::CommandsError::generic(e.to_string()))
}

/// Delete the legacy `"disabled"` field from `name`'s entry in `path`.
async fn strip_legacy_disabled(name: &str, path: &Path) -> crate::Result<()> {
    let mut parsed = read_json(path).await?;
    if let Some(entry) = parsed
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .and_then(|servers| servers.get_mut(name))
        .and_then(|entry| entry.as_object_mut())
    {
        entry.remove("disabled");
        write_json(path, &parsed).await?;
    }
    Ok(())
}

/// Report a definition that survives a `/mcp remove`.
///
/// Asks the loader, so it only fires when a *lower*-precedence file defines the
/// same name: the merge falls back to that definition and the removal alone
/// doesn't stop the server loading.
fn residual_note(name: &str, paths: &McpPaths) -> Option<String> {
    let survivor = McpConfigLoader::load_with_roots(paths.roots(), &paths.config_home)
        .into_iter()
        .find(|server| server.name == name)?;
    Some(format!(
        "\n\nNote: '{name}' is also defined at {} scope and will keep loading from there. \
         Use /mcp disable {name} to stop it regardless of where it is defined.",
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

/// Refuse to override enterprise/managed policy, in either direction: their
/// files are not editable from `/mcp`, and the user toggle must not switch
/// off an admin-mandated server.
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
