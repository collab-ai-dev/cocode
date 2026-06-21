//! Shell theme integration.
//!
//! The config-free `Theme`/`ThemeName` palette types live in `coco-tui-ui`
//! (so widgets can consume them without an app dependency). This module owns the
//! shell-only loader (`settings.json` / `~/.coco/theme.json`) and the file
//! watcher, and re-exports the palette types for existing call sites.

pub use coco_tui_ui::theme::Theme;
pub use coco_tui_ui::theme::ThemeName;

mod config;
mod watcher;

pub use config::PartialThemeColors;
pub use config::ThemeChoice;
pub use config::ThemeConfig;
pub use config::ThemeDefinition;
pub use config::ThemeLoadResult;
pub use config::ThemeMode;
pub use config::ThemeRegistry;
pub use config::ThemeRuntimeState;
pub use config::ThemeSetting;
pub use config::load_theme_runtime_or_default;
pub use config::save_theme_setting;
pub use config::theme_config_path;
pub use watcher::ThemeSetup;
pub use watcher::ThemeWatcher;
pub use watcher::install_theme;

#[cfg(test)]
#[path = "theme.test.rs"]
mod tests;
