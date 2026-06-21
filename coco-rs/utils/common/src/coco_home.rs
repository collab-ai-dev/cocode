//! Shared coco home directory resolution.

use std::path::PathBuf;

/// Environment variable for overriding the coco config / home directory.
///
/// Kept in sync with `coco_config::EnvKey::CocoConfigDir`, but duplicated
/// as a literal here because `coco-utils-common` sits below `coco-config`
/// in the dependency graph and cannot reach back up.
pub const COCO_CONFIG_DIR_ENV: &str = "COCO_CONFIG_DIR";

/// Default coco directory name.
const DEFAULT_DIR: &str = ".coco";

/// Resolve the coco home directory.
///
/// Checks `COCO_CONFIG_DIR` env var first, falls back to `~/.coco`.
/// If no home directory can be determined, falls back to `./.coco`.
pub fn find_coco_home() -> PathBuf {
    std::env::var(COCO_CONFIG_DIR_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(DEFAULT_DIR)
        })
}

#[cfg(test)]
#[path = "coco_home.test.rs"]
mod tests;
