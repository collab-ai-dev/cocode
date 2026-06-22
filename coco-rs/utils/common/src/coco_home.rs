//! Shared coco home directory resolution.

use std::path::PathBuf;

/// Environment variable for overriding the coco config / home directory.
///
/// Kept in sync with `coco_config::EnvKey::CocoConfigDir`, but duplicated
/// as a literal here because `coco-utils-common` sits below `coco-config`
/// in the dependency graph and cannot reach back up.
pub const COCO_CONFIG_DIR_ENV: &str = "COCO_CONFIG_DIR";

/// Default config directory name.
pub const COCO_CONFIG_DIR_NAME: &str = ".cocode";

/// Resolve the coco home directory.
///
/// Checks `COCO_CONFIG_DIR` env var first, then falls back to the default
/// config directory under the user's home directory.
pub fn find_coco_home() -> PathBuf {
    std::env::var(COCO_CONFIG_DIR_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(COCO_CONFIG_DIR_NAME)
        })
}

#[cfg(test)]
#[path = "coco_home.test.rs"]
mod tests;
