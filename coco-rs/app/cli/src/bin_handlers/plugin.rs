//! `coco plugin <action>` — plugin install/uninstall/list/validate.
//!
//! Implements list, install-from-path, install-from-marketplace (via the
//! shared `coco_plugins::marketplace::MarketplaceManager`), uninstall,
//! validate. Scopes and lockfiles are a follow-up.

use anyhow::Result;

use coco_cli::PluginAction;
use coco_config::global_config;

pub async fn run_plugin_subcommand(action: &PluginAction) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_home = global_config::config_home();

    match action {
        PluginAction::List => {
            // Same enabled set the session bootstrap registers contributions
            // from — marketplace versioned cache + local `inline` dirs, gated by
            // settings.json `enabled_plugins`.
            let mut plugins = coco_plugins::load_enabled_plugins(&config_home, &cwd);
            if plugins.is_empty() {
                println!("No plugins installed.");
                return Ok(());
            }
            println!("Installed plugins:");
            plugins.sort_by(|a, b| a.id.name.cmp(&b.id.name));
            for plugin in &plugins {
                let version = plugin.manifest.version.as_deref().unwrap_or("—");
                let source = match &plugin.load_source {
                    coco_plugins::loader::PluginLoadSource::Marketplace { marketplace } => {
                        format!("marketplace {marketplace}")
                    }
                    coco_plugins::loader::PluginLoadSource::SessionDir => "local".into(),
                    coco_plugins::loader::PluginLoadSource::Builtin => "builtin".into(),
                };
                let desc = plugin.manifest.description.as_deref().unwrap_or("");
                println!(
                    "  {name} {version} ({source})  — {desc}",
                    name = plugin.id.name,
                );
            }
            Ok(())
        }
        PluginAction::Install { name } => {
            let src = std::path::Path::new(name);
            if !src.is_dir() {
                // Not a local path — try marketplace install. The slash
                // command (`/plugin install`) shares the same underlying
                // `MarketplaceManager::install_plugin`, so the two paths
                // accept the same `name[@marketplace]` syntax.
                return install_from_marketplace(name, &config_home).await;
            }
            if !src.join("PLUGIN.toml").is_file() && !src.join("plugin.json").is_file() {
                anyhow::bail!("'{name}' does not contain a PLUGIN.toml or plugin.json manifest");
            }
            // Load + validate via the V2 loader (the single manifest reader).
            let loader = coco_plugins::loader::PluginLoader::new(config_home.join("plugins"));
            let plugin = loader
                .load_from_dir(
                    src,
                    coco_plugins::loader::PluginLoadSource::SessionDir,
                    None,
                )
                .map_err(|e| anyhow::anyhow!("invalid plugin manifest: {}", e.message))?;
            let plugin_name = plugin.manifest.name;
            // Reject manifest names that could traverse the install root.
            // `Path::join` treats "../" literally and does not escape the root on
            // disk, but a normalized `..` chain can still confuse audit tooling.
            if plugin_name.is_empty()
                || plugin_name.contains('/')
                || plugin_name.contains('\\')
                || plugin_name == ".."
                || plugin_name == "."
            {
                anyhow::bail!(
                    "plugin manifest name '{plugin_name}' contains path separators or reserved \
                     component; refusing to install"
                );
            }
            let dest_root = config_home.join("plugins");
            std::fs::create_dir_all(&dest_root)?;
            let dest = dest_root.join(&plugin_name);
            if dest.exists() {
                anyhow::bail!(
                    "plugin '{plugin_name}' already installed at {}; uninstall first",
                    dest.display()
                );
            }
            copy_dir_recursive(src, &dest)?;
            println!("Installed plugin '{plugin_name}' → {}", dest.display());
            Ok(())
        }
        PluginAction::Uninstall { name } => {
            let dest = config_home.join("plugins").join(name);
            if !dest.is_dir() {
                anyhow::bail!("plugin '{name}' is not installed at {}", dest.display());
            }
            std::fs::remove_dir_all(&dest)?;
            println!("Uninstalled plugin '{name}'");
            Ok(())
        }
        PluginAction::Validate { path } => {
            let path = std::path::Path::new(path);
            // The V2 loader reads the manifest from a directory; accept either a
            // dir or a path to the manifest file itself.
            let dir = if path.is_file() {
                path.parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| path.to_path_buf())
            } else {
                path.to_path_buf()
            };
            let loader = coco_plugins::loader::PluginLoader::new(config_home.join("plugins"));
            let plugin = loader
                .load_from_dir(
                    &dir,
                    coco_plugins::loader::PluginLoadSource::SessionDir,
                    None,
                )
                .map_err(|e| anyhow::anyhow!("{}", e.message))?;
            let m = &plugin.manifest;
            println!("✓ {} v{}", m.name, m.version.as_deref().unwrap_or("—"));
            if let Some(desc) = &m.description {
                println!("  {desc}");
            }
            let mut parts = Vec::new();
            if m.skills.is_some() {
                parts.push("skills");
            }
            if m.hooks.is_some() {
                parts.push("hooks");
            }
            if m.agents.is_some() {
                parts.push("agents");
            }
            if m.commands.is_some() {
                parts.push("commands");
            }
            if m.mcp_servers.is_some() {
                parts.push("mcp_servers");
            }
            if m.lsp_servers.is_some() {
                parts.push("lsp_servers");
            }
            if !parts.is_empty() {
                println!("  contributes: {}", parts.join(", "));
            }
            Ok(())
        }
    }
}

/// Recursively copy `src` into `dst`. Used by plugin install.
///
/// Symlinks are skipped with a warning — following them lets a hostile plugin
/// exfiltrate host files (e.g. `~/.ssh/id_rsa`) into the install tree. Use
/// `symlink_metadata()` so the check doesn't follow; `file_type().is_dir()`
/// and `is_file()` otherwise follow by default.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let meta = std::fs::symlink_metadata(&src_path)?;
        let ty = meta.file_type();
        if ty.is_symlink() {
            eprintln!(
                "warning: skipping symlink in plugin source: {}",
                src_path.display()
            );
            continue;
        }
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else if ty.is_file() {
            std::fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

/// Install a plugin from a known marketplace.
///
/// Both the CLI and the `/plugin install <name>[@<marketplace>]` slash command
/// funnel through [`coco_plugins::install::install_plugin_from_marketplace`]
/// so accepted syntax and error semantics stay in lock-step.
async fn install_from_marketplace(target: &str, config_home: &std::path::Path) -> Result<()> {
    let plugins_dir = config_home.join("plugins");
    let policy = coco_plugins::security::EnterprisePolicy::from_managed_settings();
    let result = coco_plugins::install::install_plugin_from_marketplace(
        &plugins_dir,
        Some(config_home),
        &policy,
        target,
        coco_plugins::schemas::PluginScope::User,
    )
    .await;
    match result {
        Ok(outcome) => {
            println!(
                "✓ Installed {plugin_name}{dep_note}. Run /reload-plugins to activate.",
                plugin_name = outcome.plugin_name,
                dep_note = outcome.dep_note,
            );
            Ok(())
        }
        Err(coco_plugins::install::InstallError::NoMarketplacesConfigured) => {
            anyhow::bail!(
                "No marketplaces configured. Run `/plugin marketplace add <source>` in an \
                 interactive session first (sources: GitHub `owner/repo`, SSH/HTTPS git URL, \
                 raw URL, or local dir)."
            );
        }
        Err(coco_plugins::install::InstallError::NotFound { plugin_name, .. }) => {
            anyhow::bail!(
                "plugin '{plugin_name}' not found in any known marketplace. \
                 Try `coco plugin list` to see registered marketplaces."
            );
        }
        Err(e @ coco_plugins::install::InstallError::BlockedByPolicy { .. })
        | Err(e @ coco_plugins::install::InstallError::DependencyBlockedByPolicy { .. })
        | Err(e @ coco_plugins::install::InstallError::ResolutionFailed(_))
        | Err(e @ coco_plugins::install::InstallError::SettingsWriteFailed(_)) => {
            anyhow::bail!("{e}")
        }
        Err(coco_plugins::install::InstallError::Other(e)) => Err(e.into()),
    }
}
