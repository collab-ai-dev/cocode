//! Binary-side subcommand handlers.
//!
//! These previously lived inline in `main.rs`. They print to stdout and
//! drive coco subsystems via their public APIs — moved here to keep
//! `main.rs` focused on the top-level mode dispatch (TUI / headless /
//! SDK / subcommand routing).

pub mod agents;
pub mod config;
pub mod plugin;
pub mod sessions;
