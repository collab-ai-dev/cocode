//! Shared utilities for codex-rs.
//!
//! This crate provides common utilities that can be used by all crates
//! in the workspace without circular dependencies.

pub mod coco_home;
pub mod elapsed;
pub mod format_env_display;
pub mod fuzzy_match;
pub mod logging;

pub use coco_home::COCO_CONFIG_DIR_ENV;
pub use coco_home::find_coco_home;
pub use elapsed::format_duration;
pub use elapsed::format_elapsed;
pub use format_env_display::format_env_display;
pub use fuzzy_match::fuzzy_indices;
pub use fuzzy_match::fuzzy_match;
pub use logging::ConfigurableTimer;
pub use logging::LoggingConfig;
pub use logging::TimezoneConfig;
pub use logging::build_env_filter;
