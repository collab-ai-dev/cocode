//! `/plugin` — plugin management (list, install, uninstall, info).
//!
//! Scans for PLUGIN.toml manifests in plugin directories, reads their
//! metadata, and displays installed plugins with their contributions.

use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

/// A discovered plugin from scanning plugin directories.
struct PluginEntry {
    name: String,
    version: Option<String>,
    description: Option<String>,
    source_dir: String,
    has_skills: bool,
    has_hooks: bool,
    has_agents: bool,
}

/// Async handler for `/plugin [list|install|uninstall|info]`.
pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let subcommand = args.trim().to_string();

        match subcommand.as_str() {
            "" | "list" => list_plugins().await,
            "search" => Ok("Usage: /plugin search <query>".to_string()),
            "enable" => Ok("Usage: /plugin enable <name>".to_string()),
            "disable" => Ok("Usage: /plugin disable <name>".to_string()),
            "marketplace" | "marketplaces" => marketplace_help().await,
            _ => {
                if let Some(name) = subcommand.strip_prefix("install ") {
                    install_plugin(name.trim()).await
                } else if let Some(name) = subcommand.strip_prefix("uninstall ") {
                    uninstall_plugin(name.trim()).await
                } else if let Some(name) = subcommand.strip_prefix("info ") {
                    plugin_info(name.trim()).await
                } else if let Some(query) = subcommand.strip_prefix("search ") {
                    search_plugins(query.trim()).await
                } else if let Some(name) = subcommand.strip_prefix("enable ") {
                    enable_plugin(name.trim()).await
                } else if let Some(name) = subcommand.strip_prefix("disable ") {
                    disable_plugin(name.trim()).await
                } else if let Some(mkt_args) = subcommand.strip_prefix("marketplace ") {
                    marketplace_subcommand(mkt_args.trim()).await
                } else {
                    Ok(plugin_usage())
                }
            }
        }
    })
}

/// CommandHandler entry point used by the interactive slash dispatcher.
///
/// Bare `/plugin`, `/plugins`, and `/marketplace` open the plugin manager
/// overlay. Explicit subcommands keep the legacy text path
/// so headless/scripted use remains stable.
pub struct PluginHandler;

#[async_trait]
impl CommandHandler for PluginHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        if args.trim().is_empty() {
            return Ok(CommandResult::OpenDialog(DialogSpec::PluginPicker));
        }
        Ok(CommandResult::Text(handler(args.to_string()).await?))
    }

    fn handler_name(&self) -> &str {
        "plugin"
    }
}

/// List all installed plugins from project and user directories.
async fn list_plugins() -> crate::Result<String> {
    let mut plugins = Vec::new();

    // Scan project plugins
    scan_plugin_dir(Path::new(".coco/plugins"), "project", &mut plugins).await;

    // Scan user plugins
    scan_plugin_dir(&resolve_plugins_dir(), "user", &mut plugins).await;

    let mut out = String::from("## Installed Plugins\n\n");

    if plugins.is_empty() {
        out.push_str("No plugins installed.\n\n");
        out.push_str("Plugin directories:\n");
        out.push_str("  .coco/plugins/         (project-level)\n");
        out.push_str("  ~/.coco/plugins/       (user-level)\n\n");
        out.push_str("Each plugin is a directory containing a PLUGIN.toml manifest.\n\n");
        out.push_str("A plugin can provide:\n");
        out.push_str("  - Skills (slash commands)\n");
        out.push_str("  - Hooks (pre/post tool event handlers)\n");
        out.push_str("  - Agents (subagent definitions)\n");
        out.push_str("  - MCP servers (tool providers)");
    } else {
        out.push_str(&format!(
            "{} plugin{} installed:\n\n",
            plugins.len(),
            if plugins.len() == 1 { "" } else { "s" },
        ));

        for plugin in &plugins {
            let version_str = plugin
                .version
                .as_deref()
                .map_or(String::new(), |v| format!(" v{v}"));

            out.push_str(&format!(
                "  {}{version_str}  ({})\n",
                plugin.name, plugin.source_dir
            ));

            if let Some(desc) = &plugin.description {
                out.push_str(&format!("    {desc}\n"));
            }

            // Contributions summary
            let mut contribs = Vec::new();
            if plugin.has_skills {
                contribs.push("skills");
            }
            if plugin.has_hooks {
                contribs.push("hooks");
            }
            if plugin.has_agents {
                contribs.push("agents");
            }
            if !contribs.is_empty() {
                out.push_str(&format!("    provides: {}\n", contribs.join(", ")));
            }
            out.push('\n');
        }
    }

    out.push_str("\nCommands:\n");
    out.push_str("  /plugin install <name>    Install a plugin\n");
    out.push_str("  /plugin uninstall <name>  Remove a plugin\n");
    out.push_str("  /plugin info <name>       Show plugin details");

    Ok(out)
}

/// Scan a plugin directory for PLUGIN.toml manifests.
async fn scan_plugin_dir(dir: &Path, source_label: &str, plugins: &mut Vec<PluginEntry>) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("PLUGIN.toml");
        if tokio::fs::metadata(&manifest_path).await.is_err() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let (version, description) = parse_plugin_manifest(&manifest_path).await;

        let has_skills = tokio::fs::metadata(path.join("skills")).await.is_ok();
        let has_hooks = tokio::fs::metadata(path.join("hooks")).await.is_ok();
        let has_agents = tokio::fs::metadata(path.join("agents")).await.is_ok();

        plugins.push(PluginEntry {
            name,
            version,
            description,
            source_dir: source_label.to_string(),
            has_skills,
            has_hooks,
            has_agents,
        });
    }
}

/// Parse version and description from a PLUGIN.toml file.
async fn parse_plugin_manifest(path: &PathBuf) -> (Option<String>, Option<String>) {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return (None, None);
    };

    // Simple TOML key-value parsing (avoids toml crate dependency)
    let mut version = None;
    let mut description = None;

    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("version") {
            if let Some(val) = extract_toml_string_value(rest) {
                version = Some(val);
            }
        } else if let Some(rest) = line.strip_prefix("description")
            && let Some(val) = extract_toml_string_value(rest)
        {
            description = Some(val);
        }
    }

    (version, description)
}

/// Extract a string value from a TOML key-value line (after the key name).
/// Expects format: ` = "value"` or ` = 'value'`.
fn extract_toml_string_value(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let rest = rest.strip_prefix('=')?;
    let rest = rest.trim();

    if let Some(inner) = rest.strip_prefix('"') {
        inner.strip_suffix('"').map(String::from)
    } else if let Some(inner) = rest.strip_prefix('\'') {
        inner.strip_suffix('\'').map(String::from)
    } else {
        Some(rest.to_string())
    }
}

/// Install a plugin from a known marketplace, recording the installation
/// in `installed_plugins.json` so the next session loads it.
///
/// Accepts either `name@marketplace` or just
/// `name` (search across all known marketplaces). If the plugin isn't
/// found in any marketplace, the handler is honest about it instead of
/// silently scaffolding an empty directory — the prior behavior misled
/// users into thinking remote plugins had been fetched.
///
/// Live-registry refresh is deferred to `/reload-plugins` (which re-runs
/// `load_enabled_plugins`); a fresh install is not auto-activated mid-session.
async fn install_plugin(target: &str) -> crate::Result<String> {
    if target.trim().is_empty() {
        return Ok("Usage: /plugin install <name>[@<marketplace>]".to_string());
    }
    let plugins_dir = resolve_plugins_dir();
    // Settings live at the config root (same as the `coco plugin` CLI), so
    // enabledPlugins is written where the loader reads it.
    let settings_dir = Some(coco_config::global_config::config_home());
    let policy = coco_plugins::security::EnterprisePolicy::from_managed_settings();
    let result = coco_plugins::install::install_plugin_from_marketplace(
        &plugins_dir,
        settings_dir.as_deref(),
        &policy,
        target,
        coco_plugins::schemas::PluginScope::User,
    )
    .await;
    match result {
        Ok(outcome) => Ok(format!(
            "{tick} Installed {plugin_name}{dep_note}. Run /reload-plugins to activate.",
            tick = '✓',
            plugin_name = outcome.plugin_name,
            dep_note = outcome.dep_note,
        )),
        Err(coco_plugins::install::InstallError::NoMarketplacesConfigured) => {
            Ok("No marketplaces configured. Add one before installing:\n\
                 \n\
                 /plugin marketplace add <source>\n\
                 \n\
                 Sources: GitHub (owner/repo), SSH/HTTPS git URL, raw URL, or local directory."
                .to_string())
        }
        Err(coco_plugins::install::InstallError::NotFound {
            plugin_name,
            marketplace_filter,
        }) => {
            let suggestion = match marketplace_filter.as_deref() {
                None => format!("/plugin search {plugin_name}"),
                Some(m) => format!("/plugin search {plugin_name} (in marketplace '{m}')"),
            };
            Ok(format!(
                "Plugin '{plugin_name}' not found in any known marketplace.\n\
                 \n\
                 Try: {suggestion}"
            ))
        }
        Err(e @ coco_plugins::install::InstallError::BlockedByPolicy { .. })
        | Err(e @ coco_plugins::install::InstallError::DependencyBlockedByPolicy { .. })
        | Err(e @ coco_plugins::install::InstallError::ResolutionFailed(_))
        | Err(e @ coco_plugins::install::InstallError::SettingsWriteFailed(_)) => Ok(e.to_string()),
        Err(coco_plugins::install::InstallError::Other(e)) => Err(e.into()),
    }
}

/// Uninstall a plugin: remove its directory AND scrub the corresponding
/// entry from `installed_plugins.json` so the next session's loader
/// doesn't try to materialise stale state.
///
/// Accepts a bare name (looks across project + user dirs) or
/// `name@marketplace` (the canonical plugin ID — matches what `install`
/// records in installed_plugins.json).
async fn uninstall_plugin(target: &str) -> crate::Result<String> {
    if target.is_empty() {
        return Ok("Usage: /plugin uninstall <name>".to_string());
    }

    let (name, mkt_filter) = match target.split_once('@') {
        Some((n, m)) => (n.trim().to_string(), Some(m.trim().to_string())),
        None => (target.to_string(), None),
    };

    let project_dir = PathBuf::from(".coco/plugins").join(&name);
    let user_dir = Some(resolve_plugins_dir().join(&name));

    let removed_path = if tokio::fs::metadata(&project_dir).await.is_ok() {
        tokio::fs::remove_dir_all(&project_dir).await?;
        Some(project_dir)
    } else if let Some(ud) = user_dir
        && tokio::fs::metadata(&ud).await.is_ok()
    {
        tokio::fs::remove_dir_all(&ud).await?;
        Some(ud)
    } else {
        None
    };

    // Scrub installed_plugins.json — covers both the local-scaffold case
    // (no entry, no-op) and marketplace-installed plugins.
    let installed_path = resolve_plugins_dir().join("installed_plugins.json");
    if tokio::fs::metadata(&installed_path).await.is_ok() {
        let plugin_id = mkt_filter
            .as_ref()
            .map(|m| format!("{name}@{m}"))
            .unwrap_or_else(|| name.clone());
        let name_owned = name.clone();
        let _ = tokio::task::spawn_blocking(move || -> crate::Result<()> {
            let mut mgr = coco_plugins::loader::InstalledPluginsManager::load(installed_path)?;
            // Try both `name` and `name@anything` — when no marketplace
            // was given, scan for any matching prefix.
            if mkt_filter.is_some() {
                mgr.remove_plugin(&plugin_id);
            } else {
                let prefix = format!("{name_owned}@");
                let to_remove: Vec<String> = mgr
                    .installed_plugin_ids()
                    .into_iter()
                    .filter(|id| id == &plugin_id || id.starts_with(&prefix))
                    .map(String::from)
                    .collect();
                for id in &to_remove {
                    mgr.remove_plugin(id);
                }
            }
            mgr.save()?;
            Ok(())
        })
        .await?;
    }

    match removed_path {
        Some(p) => Ok(format!(
            "Uninstalled plugin '{name}' from {}.\n\
             Run /reload-plugins to apply (slash commands refresh \
             immediately; skills, hooks, agents, MCP servers, and tools \
             still pick up on next session restart).",
            p.display()
        )),
        None => Ok(format!("Plugin '{name}' not found.")),
    }
}

/// Show detailed information about a specific plugin.
async fn plugin_info(name: &str) -> crate::Result<String> {
    let project_dir = PathBuf::from(".coco/plugins").join(name);
    let user_dir = Some(resolve_plugins_dir().join(name));

    let plugin_dir = if tokio::fs::metadata(&project_dir).await.is_ok() {
        project_dir
    } else if let Some(ref ud) = user_dir {
        if tokio::fs::metadata(ud).await.is_ok() {
            ud.clone()
        } else {
            return Ok(format!("Plugin '{name}' not found."));
        }
    } else {
        return Ok(format!("Plugin '{name}' not found."));
    };

    let manifest_path = plugin_dir.join("PLUGIN.toml");
    let (version, description) = parse_plugin_manifest(&manifest_path).await;

    let mut out = format!("## Plugin: {name}\n\n");
    out.push_str(&format!("  Location:    {}\n", plugin_dir.display()));
    out.push_str(&format!(
        "  Version:     {}\n",
        version.as_deref().unwrap_or("unknown")
    ));
    out.push_str(&format!(
        "  Description: {}\n",
        description.as_deref().unwrap_or("(none)")
    ));

    // List contents
    let has_skills = tokio::fs::metadata(plugin_dir.join("skills")).await.is_ok();
    let has_hooks = tokio::fs::metadata(plugin_dir.join("hooks")).await.is_ok();
    let has_agents = tokio::fs::metadata(plugin_dir.join("agents")).await.is_ok();

    out.push_str("\n  Contributions:\n");
    out.push_str(&format!(
        "    Skills:  {}\n",
        if has_skills { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "    Hooks:   {}\n",
        if has_hooks { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "    Agents:  {}",
        if has_agents { "yes" } else { "no" }
    ));

    Ok(out)
}

/// Usage text for the /plugin command.
fn plugin_usage() -> String {
    "Plugin Management\n\n\
     Usage:\n\
     /plugin                    List installed plugins\n\
     /plugin install <name>     Install a plugin\n\
     /plugin uninstall <name>   Remove a plugin\n\
     /plugin info <name>        Show plugin details\n\
     /plugin search <query>     Search available plugins\n\
     /plugin enable <name>      Enable a disabled plugin\n\
     /plugin disable <name>     Disable a plugin\n\
     /plugin marketplace        Manage marketplace sources\n\n\
     Marketplace:\n\
     /plugin marketplace list   List configured marketplaces\n\
     /plugin marketplace add    Add a marketplace source\n\
     /plugin marketplace remove Remove a marketplace"
        .to_string()
}

/// Search for plugins across marketplaces.
async fn search_plugins(query: &str) -> crate::Result<String> {
    if query.is_empty() {
        return Ok("Usage: /plugin search <query>".to_string());
    }

    let plugins_dir = resolve_plugins_dir();
    let manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);

    let results = manager.search_plugins(query);

    if results.is_empty() {
        return Ok(format!(
            "No plugins found matching \"{query}\".\n\n\
             Tip: Add a marketplace with `/plugin marketplace add <source>`"
        ));
    }

    let mut out = format!("## Search results for \"{query}\"\n\n");
    for plugin in results.iter().take(20) {
        out.push_str(&format!("  {}", plugin.name));
        if let Some(ref v) = plugin.version {
            out.push_str(&format!(" v{v}"));
        }
        out.push_str(&format!("  @{}", plugin.marketplace));
        if plugin.downloads > 0 {
            out.push_str(&format!("  ({} installs)", plugin.downloads));
        }
        out.push('\n');
        if let Some(ref desc) = plugin.description {
            out.push_str(&format!("    {desc}\n"));
        }
    }

    if results.len() > 20 {
        out.push_str(&format!("\n... and {} more", results.len() - 20));
    }

    out.push_str("\n\nInstall: /plugin install <name>@<marketplace>");
    Ok(out)
}

/// Settings directory where `enabled_plugins` lives (config root, same place
/// the install path and loader read/write).
fn settings_dir_for_plugins() -> PathBuf {
    coco_config::global_config::config_home()
}

/// Enable a plugin: set `enabled_plugins[<id>].enabled = true` in settings.json
/// — the single source of truth the loader and policy layer read.
/// The orphaned `disabled_plugins.json` is gone.
async fn enable_plugin(name: &str) -> crate::Result<String> {
    if name.is_empty() {
        return Ok("Usage: /plugin enable <name>".to_string());
    }
    let Some(plugin_id) = resolve_installed_plugin_id(name) else {
        return Ok(format!("Plugin '{name}' not found."));
    };
    // Enterprise-policy guard: org-blocked plugins cannot be enabled at any
    // scope. Same gate as `install_plugin`'s blocklist (checks
    // `isPluginBlockedByPolicy` on enable only — disable is intentionally
    // ungated). Checked after the dir-exists check so we only block plugins
    // the user could otherwise enable.
    let policy = coco_plugins::security::EnterprisePolicy::from_managed_settings();
    if let coco_plugins::security::PolicyVerdict::BlockedPlugin { plugin } =
        coco_plugins::security::check_policy(&plugin_id, /*is_user_scope*/ true, &policy)
    {
        return Err(crate::CommandsError::generic(format!(
            "Plugin \"{plugin}\" is blocked by your organization's policy and cannot be enabled"
        )));
    }
    let settings_dir = settings_dir_for_plugins();
    // Enabled by default (no entry) or already explicitly enabled.
    if coco_plugins::install::read_plugin_enabled(&settings_dir, &plugin_id) != Some(false) {
        return Ok(format!("Plugin '{name}' is already enabled."));
    }
    coco_plugins::install::set_plugin_enabled(&settings_dir, &plugin_id, true).map_err(|e| {
        crate::CommandsError::generic(format!("failed to update settings.json: {e}"))
    })?;
    Ok(format!(
        "Plugin '{name}' enabled (persisted). Run /reload-plugins to \
         apply (slash commands refresh immediately; skills, hooks, \
         agents, MCP servers, and tools still pick up on next session \
         restart)."
    ))
}

/// Disable a plugin: set `enabled_plugins[<id>].enabled = false` in
/// settings.json. Same source of truth as enable/install.
async fn disable_plugin(name: &str) -> crate::Result<String> {
    if name.is_empty() {
        return Ok("Usage: /plugin disable <name>".to_string());
    }
    let Some(plugin_id) = resolve_installed_plugin_id(name) else {
        return Ok(format!("Plugin '{name}' not found."));
    };
    let settings_dir = settings_dir_for_plugins();
    if coco_plugins::install::read_plugin_enabled(&settings_dir, &plugin_id) == Some(false) {
        return Ok(format!("Plugin '{name}' is already disabled."));
    }
    coco_plugins::install::set_plugin_enabled(&settings_dir, &plugin_id, false).map_err(|e| {
        crate::CommandsError::generic(format!("failed to update settings.json: {e}"))
    })?;
    Ok(format!(
        "Plugin '{name}' disabled (persisted). Run /reload-plugins to \
         apply (slash commands refresh immediately; skills, hooks, \
         agents, MCP servers, and tools still pick up on next session \
         restart)."
    ))
}

/// Resolve a user-supplied plugin name to its full installed identity so the
/// persisted `enabled_plugins` key matches exactly what the loader reads.
///
/// Searches *every* installed plugin (inline standing-dir plugins keyed
/// `<name>@inline` **and** marketplace-installed plugins in the versioned
/// cache, enabled or disabled), matching either the full id (`foo@mkt`) or a
/// bare name (`foo`). Returns `None` when nothing is installed under that name
/// — so `/plugin enable|disable` refuses to silently mutate state for unknown
/// names, and now works for marketplace plugins the standing-dir scan can't
/// see.
fn resolve_installed_plugin_id(name: &str) -> Option<coco_plugins::identifier::PluginId> {
    let config_home = settings_dir_for_plugins();
    let cwd = std::env::current_dir().unwrap_or_default();
    coco_plugins::load_all_installed_plugins(&config_home, &cwd)
        .into_iter()
        .find(|p| p.id.to_string() == name || p.id.name == name)
        .map(|p| coco_plugins::identifier::PluginId::parse(&p.id.to_string()))
}

/// Marketplace subcommand dispatcher.
async fn marketplace_subcommand(args: &str) -> crate::Result<String> {
    match args {
        "" | "list" => marketplace_list().await,
        "add" => Ok("Usage: /plugin marketplace add <source>\n\n\
             Sources:\n\
             - GitHub:    owner/repo\n\
             - Git URL:   https://github.com/org/marketplace.git\n\
             - HTTP URL:  https://example.com/marketplace.json\n\
             - Local dir: /path/to/marketplace/"
            .to_string()),
        _ => {
            if let Some(name) = args.strip_prefix("remove ") {
                marketplace_remove(name.trim()).await
            } else if let Some(source) = args.strip_prefix("add ") {
                marketplace_add(source.trim()).await
            } else if let Some(name) = args.strip_prefix("update ") {
                marketplace_update(name.trim()).await
            } else if args == "update" {
                marketplace_update_all().await
            } else {
                marketplace_help().await
            }
        }
    }
}

/// Show marketplace help.
async fn marketplace_help() -> crate::Result<String> {
    Ok("Marketplace Management\n\n\
        Usage:\n\
        /plugin marketplace list              List configured marketplaces\n\
        /plugin marketplace add <source>      Add and fetch a marketplace source\n\
        /plugin marketplace update [<name>]   Re-fetch one or all marketplaces\n\
        /plugin marketplace remove <name>     Remove a marketplace"
        .to_string())
}

/// List configured marketplaces.
async fn marketplace_list() -> crate::Result<String> {
    let plugins_dir = resolve_plugins_dir();
    let manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);
    let known = manager.load_known_marketplaces();

    if known.is_empty() {
        return Ok("No marketplaces configured.\n\n\
             Add one: /plugin marketplace add <source>"
            .to_string());
    }

    let mut out = format!("## Configured Marketplaces ({})\n\n", known.len());
    for (name, entry) in &known {
        let official = if coco_plugins::marketplace::is_official_marketplace_name(name) {
            " (official)"
        } else {
            ""
        };
        out.push_str(&format!("  {name}{official}\n"));
        out.push_str(&format!("    source: {:?}\n", entry.source));
        out.push_str(&format!("    updated: {}\n\n", entry.last_updated));
    }

    Ok(out)
}

/// Add a marketplace source.
///
/// Input parsing: SSH git URLs, HTTP/HTTPS (`.git` / Azure `/_git/` → Git
/// source; github.com → Git with `.git` appended; everything else → Url
/// source), local paths, and `owner/repo[#ref|@ref]` shorthand.
async fn marketplace_add(source: &str) -> crate::Result<String> {
    if source.trim().is_empty() {
        return Ok("Usage: /plugin marketplace add <source>".to_string());
    }

    let mkt_source = match coco_plugins::parse_marketplace_input::parse_marketplace_input(
        source,
        dirs::home_dir,
    ) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Ok(format!(
                "Unrecognised marketplace source: '{source}'\n\n\
                 Supported forms:\n  \
                 owner/repo[#ref|@ref]            (GitHub shorthand)\n  \
                 git@host:path[.git][#ref]        (SSH)\n  \
                 https://host/repo.git[#ref]       (HTTPS git)\n  \
                 https://host/_git/repo            (Azure DevOps)\n  \
                 https://github.com/owner/repo     (GitHub HTTPS)\n  \
                 https://host/marketplace.json     (raw JSON URL)\n  \
                 /path/to/dir | ./relative | ~/x   (local directory or .json file)"
            ));
        }
        Err(e) => return Ok(format!("Failed to add marketplace: {e}")),
    };
    let name = coco_plugins::parse_marketplace_input::derive_marketplace_name(&mkt_source);

    let plugins_dir = resolve_plugins_dir();
    let mut manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir.clone());
    let cache_dir = plugins_dir.join("marketplaces");

    // Materialize the source: git clone for github/git, HTTP download for url,
    // no-op (path returned as-is) for local file/directory sources. The fetch
    // determines the real `install_location` (a clone dir, a `<name>.json`
    // file, or a `dir/path` subpath).
    let install_location =
        match coco_plugins::fetch::fetch_marketplace(&mkt_source, &name, &cache_dir).await {
            Ok(loc) => loc.to_string_lossy().to_string(),
            Err(e) => return Ok(format!("Failed to fetch marketplace '{name}': {e}")),
        };

    match manager.register_marketplace(&name, mkt_source, &install_location) {
        // Only claim the marketplace is usable once its manifest actually loads.
        Ok(()) => match manager.load_cached_marketplace(&name) {
            Ok(_) => Ok(format!(
                "Marketplace '{name}' added. It is ready to search and install from."
            )),
            Err(e) => Ok(format!(
                "Marketplace '{name}' added, but no valid marketplace manifest was found at \
                 {install_location}: {e}"
            )),
        },
        Err(e) => Ok(format!("Failed to add marketplace: {e}")),
    }
}

/// Re-fetch a single marketplace (git pull / HTTP re-download) and reload it.
async fn marketplace_update(name: &str) -> crate::Result<String> {
    if name.is_empty() {
        return Ok("Usage: /plugin marketplace update <name>".to_string());
    }
    let plugins_dir = resolve_plugins_dir();
    let mut manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir.clone());

    let source = {
        let known = manager.load_known_marketplaces();
        match known.get(name) {
            Some(entry) => entry.source.clone(),
            None => {
                return Ok(format!(
                    "Marketplace '{name}' not found. Add it first: /plugin marketplace add <source>"
                ));
            }
        }
    };

    let cache_dir = plugins_dir.join("marketplaces");
    let install_location =
        match coco_plugins::fetch::fetch_marketplace(&source, name, &cache_dir).await {
            Ok(loc) => loc.to_string_lossy().to_string(),
            Err(e) => return Ok(format!("Failed to update marketplace '{name}': {e}")),
        };
    // Re-register to refresh `last_updated` + `install_location`.
    if let Err(e) = manager.register_marketplace(name, source, &install_location) {
        return Ok(format!("Failed to update marketplace '{name}': {e}"));
    }
    match manager.load_cached_marketplace(name) {
        Ok(_) => Ok(format!("Marketplace '{name}' updated.")),
        Err(e) => Ok(format!(
            "Marketplace '{name}' re-fetched, but its manifest failed to load: {e}"
        )),
    }
}

/// Re-fetch every configured marketplace.
/// Per-marketplace failures are collected and reported, not fatal.
async fn marketplace_update_all() -> crate::Result<String> {
    let plugins_dir = resolve_plugins_dir();
    let manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);
    let known = manager.load_known_marketplaces();
    if known.is_empty() {
        return Ok("No marketplaces configured.".to_string());
    }
    let mut names: Vec<String> = known.keys().cloned().collect();
    names.sort();
    let mut updated = 0usize;
    let mut failures = Vec::new();
    for name in &names {
        match marketplace_update(name).await {
            Ok(msg) if msg.starts_with("Failed") => failures.push(format!("  {name}: {msg}")),
            Ok(_) => updated += 1,
            Err(e) => failures.push(format!("  {name}: {e}")),
        }
    }
    let mut out = format!("Updated {updated}/{} marketplace(s).", names.len());
    if !failures.is_empty() {
        out.push_str("\n\nFailures:\n");
        out.push_str(&failures.join("\n"));
    }
    Ok(out)
}

/// Remove a marketplace.
async fn marketplace_remove(name: &str) -> crate::Result<String> {
    if name.is_empty() {
        return Ok("Usage: /plugin marketplace remove <name>".to_string());
    }

    let plugins_dir = resolve_plugins_dir();
    let manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);
    let mut known = manager.load_known_marketplaces();

    if known.remove(name).is_some() {
        manager.save_known_marketplaces(&known)?;
        Ok(format!("Marketplace '{name}' removed."))
    } else {
        Ok(format!("Marketplace '{name}' not found."))
    }
}

/// Resolve the user plugins directory path.
///
/// MUST share the config root with the `coco plugin` CLI subcommand
/// (`bin_handlers/plugin.rs` → `global_config::config_home().join("plugins")`).
/// The slash handlers previously hardcoded `~/.coco/plugins`, which (a)
/// ignored `$COCO_CONFIG_DIR` and (b) split-brained against the CLI's
/// `~/.coco/plugins`: a marketplace added via `/plugin` was invisible to
/// `coco plugin install` and vice-versa.
fn resolve_plugins_dir() -> PathBuf {
    coco_config::global_config::config_home().join("plugins")
}

#[cfg(test)]
#[path = "plugin.test.rs"]
mod tests;
