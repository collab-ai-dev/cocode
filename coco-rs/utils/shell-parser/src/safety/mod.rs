//! Argv-based command safety analysis.
//!
//! Ported from codex-rs/shell-command/src/command_safety/.

mod is_dangerous_command;
mod is_read_only_pipeline;
mod is_safe_command;

pub use is_dangerous_command::command_might_be_dangerous;
pub use is_read_only_pipeline::is_read_only_pipeline;
pub use is_safe_command::is_known_safe_command;
