//! MCP config file watching.
//!
//! Uses `coco-file-watch` to monitor `.mcp.json` files for changes and
//! trigger server reconnection when configs are modified.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use coco_error::BoxedError;
use coco_error::StatusCode;
use coco_error::boxed;
use coco_file_watch::FileWatcherBuilder;
use coco_file_watch::RecursiveMode;
use tokio::sync::broadcast;
use tracing::info;

/// Event emitted when MCP config files change.
#[derive(Debug, Clone)]
pub struct McpConfigChanged {
    pub paths: Vec<PathBuf>,
}

/// Watch MCP config files for changes.
///
/// Monitors:
/// - `<config_home>/mcp.json` (user-level)
/// - `.coco/mcp.json` (project-level)
/// - `.mcp.json` (project-level, legacy)
///
/// Returns a broadcast receiver that emits `McpConfigChanged` on changes.
pub fn watch_mcp_configs(
    config_home: &Path,
    project_root: Option<&Path>,
) -> Result<broadcast::Receiver<McpConfigChanged>, BoxedError> {
    let watcher: coco_file_watch::FileWatcher<McpConfigChanged> = FileWatcherBuilder::new()
        .throttle_interval(Duration::from_millis(500))
        .build(
            |event: &notify::Event| {
                let paths: Vec<PathBuf> = event
                    .paths
                    .iter()
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| n == "mcp.json" || n == ".mcp.json")
                    })
                    .cloned()
                    .collect();
                if paths.is_empty() {
                    None
                } else {
                    Some(McpConfigChanged { paths })
                }
            },
            |mut acc: McpConfigChanged, new: McpConfigChanged| {
                acc.paths.extend(new.paths);
                acc
            },
        )
        .map_err(|e| boxed(e, StatusCode::IoError))?;

    // Watch user-level config directory
    if config_home.exists() {
        watcher.watch(config_home.to_path_buf(), RecursiveMode::NonRecursive);
        info!("watching MCP user config: {}", config_home.display());
    }

    // Watch project-level configs
    if let Some(root) = project_root {
        let coco_dir = root.join(".coco");
        if coco_dir.exists() {
            watcher.watch(coco_dir.clone(), RecursiveMode::NonRecursive);
            info!("watching MCP project config: {}", coco_dir.display());
        }
        // Legacy .mcp.json at project root
        watcher.watch(root.to_path_buf(), RecursiveMode::NonRecursive);
    }

    let rx = watcher.subscribe();

    // Keep watcher alive by leaking it (it runs in background).
    // In production, store the watcher handle in McpConnectionManager.
    std::mem::forget(watcher);

    Ok(rx)
}
