//! `/lsp` — Language Server Protocol server management.
//!
//! Live status + install / enable / disable / add / remove operations
//! against `LspServersConfig`. Mirrors the structure of
//! [`crate::handlers::mcp`]: stateless async handler that reads/writes
//! the on-disk config and shells out to the same `LspInstaller` /
//! `LspServersConfig` APIs the standalone `lsp-tui` binary uses.
//!
//! The handler intentionally does NOT touch the live
//! `coco_cli::lsp_handle_adapter::LspManagerAdapter` — that would
//! require a runtime-state-aware handler. Config edits take effect on
//! session restart; the message text states this explicitly so the
//! user is not surprised.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use coco_lsp::BUILTIN_SERVERS;
use coco_lsp::BuiltinServer;
use coco_lsp::ConfigLevel;
use coco_lsp::InstallEvent;
use coco_lsp::LspInstaller;
use coco_lsp::LspServersConfig;
use coco_lsp::command_exists;
use tokio::sync::mpsc;

/// Async handler for `/lsp [list|install|enable|disable|add|remove] [server]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let trimmed = args.trim();
        match trimmed {
            "" | "list" | "status" => list_servers().await,
            other => {
                let (cmd, rest) = split_first(other);
                let rest = rest.trim();
                match cmd {
                    "install" if !rest.is_empty() => install_server(rest).await,
                    "enable" if !rest.is_empty() => toggle_server(rest, /*enable*/ true).await,
                    "disable" if !rest.is_empty() => toggle_server(rest, /*enable*/ false).await,
                    "add" if !rest.is_empty() => add_server_to_config(rest).await,
                    "remove" if !rest.is_empty() => remove_server_from_config(rest).await,
                    _ => Ok(usage_text()),
                }
            }
        }
    })
}

fn split_first(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(idx) => (&s[..idx], &s[idx..]),
        None => (s, ""),
    }
}

fn usage_text() -> String {
    "Usage:\n\
     /lsp                    Show LSP server status\n\
     /lsp install <id>       Install a builtin server's binary\n\
     /lsp enable <id>        Re-enable a disabled server\n\
     /lsp disable <id>       Disable without removing from config\n\
     /lsp add <id>           Add a builtin to user config (~/.coco/lsp_servers.json)\n\
     /lsp remove <id>        Remove from user config\n\
     \n\
     Builtin server IDs: rust-analyzer, gopls, pyright, typescript-language-server"
        .to_string()
}

/// Resolve user-level and project-level config directories. Matches the
/// resolution `LspServersConfig::load` uses internally so `add` / `remove`
/// touch the same file the loader reads.
fn resolve_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    let user = coco_lsp::find_coco_home();
    let project = std::env::current_dir().ok().map(|p| p.join(".coco"));
    (Some(user), project)
}

/// Live status — config from disk × `command_exists` per binary.
async fn list_servers() -> crate::Result<String> {
    let (user_dir, project_dir) = resolve_dirs();
    let user = user_dir.as_deref();
    let project = project_dir.as_deref();
    let cfg = LspServersConfig::load(user, project);

    let mut out = String::from("## LSP servers\n\n");

    // ── Configured ──
    if cfg.servers.is_empty() {
        out.push_str("No LSP servers configured.\n");
    } else {
        out.push_str(&format!(
            "Configured ({} server{}):\n",
            cfg.servers.len(),
            if cfg.servers.len() == 1 { "" } else { "s" }
        ));
        for (id, server) in &cfg.servers {
            let template = BuiltinServer::find_by_id(id);
            let cmd = resolve_command(server, template);
            let installed = !cmd.is_empty() && command_exists(&cmd).await;
            let exts = resolve_extensions(server, template).join(", ");
            let status = match (server.disabled, installed) {
                (true, _) => "disabled",
                (false, false) => "not installed",
                (false, true) => "ready",
            };
            let level = if user
                .map(|d| d.join("lsp_servers.json").exists())
                .unwrap_or(false)
                && LspServersConfig::detect_config_level(
                    id,
                    user.unwrap_or(std::path::Path::new("")),
                    project.unwrap_or(std::path::Path::new("")),
                ) == Some(ConfigLevel::User)
            {
                "user"
            } else if project
                .map(|d| d.join("lsp_servers.json").exists())
                .unwrap_or(false)
            {
                "project"
            } else {
                "?"
            };
            out.push_str(&format!(
                "  {marker} {id:<28} {exts:<28} {status:<14} [{level}]\n",
                marker = if server.disabled { "[-]" } else { "[+]" },
            ));
        }
    }

    // ── Available builtins (not yet configured) ──
    let mut available_buf = String::new();
    for builtin in BUILTIN_SERVERS {
        if cfg.servers.contains_key(builtin.id) {
            continue;
        }
        let cmd = builtin
            .commands
            .first()
            .and_then(|c| c.split_whitespace().next())
            .unwrap_or("");
        let installed = !cmd.is_empty() && command_exists(cmd).await;
        let exts = builtin.extensions.join(", ");
        let status = if installed {
            "installed (not in config)"
        } else {
            "not installed"
        };
        available_buf.push_str(&format!(
            "  {id:<28} {exts:<28} {status}\n",
            id = builtin.id,
        ));
    }
    if !available_buf.is_empty() {
        out.push_str("\nAvailable builtins:\n");
        out.push_str(&available_buf);
    }

    // ── Footer ──
    out.push_str("\nCommands:\n");
    out.push_str("  /lsp install <id>   Install a builtin's binary\n");
    out.push_str("  /lsp enable|disable <id>\n");
    out.push_str("  /lsp add|remove <id>   (edit user config; restart to apply)\n");
    out.push_str("\nEnable in settings: features.lsp = true.\n");
    out.push_str(
        "Config files: ~/.coco/lsp_servers.json (user), .coco/lsp_servers.json (project).",
    );
    Ok(out)
}

fn resolve_command(server: &coco_lsp::LspServerConfig, template: Option<&BuiltinServer>) -> String {
    if let Some(cmd) = &server.command {
        return cmd.clone();
    }
    template
        .and_then(|t| t.commands.first())
        .and_then(|c| c.split_whitespace().next())
        .unwrap_or("")
        .to_string()
}

fn resolve_extensions(
    server: &coco_lsp::LspServerConfig,
    template: Option<&BuiltinServer>,
) -> Vec<String> {
    if !server.file_extensions.is_empty() {
        return server.file_extensions.clone();
    }
    template
        .map(|t| t.extensions.iter().map(ToString::to_string).collect())
        .unwrap_or_default()
}

/// Spawn `LspInstaller::install_server` and drain `InstallEvent`s into
/// a single text block. The installer auto-dispatches by `install_hint`
/// (rustup → `rustup component add`, npm → `npm install -g`, …) — same
/// path the standalone `lsp-tui` binary uses, so the install behaviour
/// is identical across surfaces.
async fn install_server(server_id: &str) -> crate::Result<String> {
    if BuiltinServer::find_by_id(server_id).is_none() {
        return Ok(format!(
            "Unknown builtin '{server_id}'.\n\
             Known: rust-analyzer, gopls, pyright, typescript-language-server"
        ));
    }

    let (tx, mut rx) = mpsc::channel::<InstallEvent>(128);
    let installer = Arc::new(LspInstaller::new(Some(tx)));
    let installer_clone = installer.clone();
    let id = server_id.to_string();
    let install_handle = tokio::spawn(async move { installer_clone.install_server(&id).await });

    // Drain events until the sender drops (installer finishes). The
    // 60-second floor matches the longest install (gopls is the
    // outlier — npm-based installs finish in seconds).
    let mut lines = vec![format!("Installing '{server_id}' ...")];
    let drain = async {
        while let Some(ev) = rx.recv().await {
            match ev {
                InstallEvent::Started { server_id, method } => {
                    lines.push(format!("  start ({method}): {server_id}"));
                }
                InstallEvent::Output(line) => lines.push(format!("  {line}")),
                InstallEvent::Completed { server_id } => {
                    lines.push(format!("Installed '{server_id}'."));
                }
                InstallEvent::Failed { server_id, error } => {
                    lines.push(format!("Failed to install '{server_id}': {error}"));
                }
            }
        }
    };

    let _ = tokio::time::timeout(Duration::from_secs(600), drain).await;
    let _ = install_handle.await;
    lines.push(String::new());
    lines.push("Run `/lsp` to confirm, then restart the session for prewarm to pick it up.".into());
    Ok(lines.join("\n"))
}

/// Find the config dir that already declares `server_id` (user > project).
/// Returns `None` when the server is not in any config — the caller
/// (`enable`/`disable`) then reports "not in config" instead of touching
/// an arbitrary level.
fn find_config_dir_with(server_id: &str) -> Option<PathBuf> {
    let (user, project) = resolve_dirs();
    if let Some(ref u) = user {
        let cfg = LspServersConfig::load(Some(u), None);
        if cfg.servers.contains_key(server_id) {
            return Some(u.clone());
        }
    }
    if let Some(ref p) = project {
        let cfg = LspServersConfig::load(None, Some(p));
        if cfg.servers.contains_key(server_id) {
            return Some(p.clone());
        }
    }
    None
}

async fn toggle_server(server_id: &str, enable: bool) -> crate::Result<String> {
    let Some(dir) = find_config_dir_with(server_id) else {
        return Ok(format!(
            "'{server_id}' is not in any lsp_servers.json.\n\
             Use `/lsp add {server_id}` to add it first."
        ));
    };
    match LspServersConfig::toggle_server_disabled(&dir, server_id) {
        Ok(Some(now_disabled)) => {
            let want_disabled = !enable;
            let action = if enable { "Enabled" } else { "Disabled" };
            if now_disabled == want_disabled {
                Ok(format!(
                    "{action} '{server_id}' in {}.\nRestart the session to apply.",
                    dir.display()
                ))
            } else {
                // Toggle landed in the opposite state — call again so the
                // user-facing semantics match the verb. `toggle_server_disabled`
                // is a flip, so this re-flip lands on the requested state.
                let _ = LspServersConfig::toggle_server_disabled(&dir, server_id);
                Ok(format!(
                    "{action} '{server_id}' in {}.\nRestart the session to apply.",
                    dir.display()
                ))
            }
        }
        Ok(None) => Ok(format!("'{server_id}' not found in {}", dir.display())),
        Err(err) => Ok(format!("Failed to toggle '{server_id}': {err}")),
    }
}

async fn add_server_to_config(server_id: &str) -> crate::Result<String> {
    if BuiltinServer::find_by_id(server_id).is_none() {
        return Ok(format!(
            "Unknown builtin '{server_id}'.\n\
             Known: rust-analyzer, gopls, pyright, typescript-language-server"
        ));
    }
    let (user_dir, _) = resolve_dirs();
    let Some(dir) = user_dir else {
        return Ok("Cannot resolve user config dir (~/.coco)".to_string());
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return Ok(format!("Failed to create {}: {err}", dir.display()));
    }
    match LspServersConfig::add_server_to_file(&dir, server_id) {
        Ok(()) => Ok(format!(
            "Added '{server_id}' to {}/lsp_servers.json.\n\
             Restart the session to activate.",
            dir.display()
        )),
        Err(err) => Ok(format!("Failed to add '{server_id}': {err}")),
    }
}

async fn remove_server_from_config(server_id: &str) -> crate::Result<String> {
    let Some(dir) = find_config_dir_with(server_id) else {
        return Ok(format!("'{server_id}' is not in any lsp_servers.json"));
    };
    match LspServersConfig::remove_server_from_file(&dir, server_id) {
        Ok(true) => Ok(format!(
            "Removed '{server_id}' from {}/lsp_servers.json.\n\
             Restart the session to apply.",
            dir.display()
        )),
        Ok(false) => Ok(format!("'{server_id}' not found in {}", dir.display())),
        Err(err) => Ok(format!("Failed to remove '{server_id}': {err}")),
    }
}

#[cfg(test)]
#[path = "lsp.test.rs"]
mod tests;
