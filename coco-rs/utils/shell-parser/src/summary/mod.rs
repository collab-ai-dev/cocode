//! Command summary extraction — human-readable classification of shell commands.
//!
//! Ported from `codex-rs/shell-command/src/parse_command.rs`.

mod parse_command;
mod powershell;
pub(crate) mod shell_invoke;

use std::path::PathBuf;

pub use parse_command::parse_command;

/// A human-readable summary of what a shell command does.
///
/// Corresponds to `ParsedCommand` in codex-rs. Renamed to avoid conflict with
/// the AST-level `ParsedShell` type.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandSummary {
    Read {
        cmd: String,
        name: String,
        /// (Best effort) Path to the file being read by the command.
        path: PathBuf,
    },
    ListFiles {
        cmd: String,
        path: Option<String>,
    },
    Search {
        cmd: String,
        query: Option<String>,
        path: Option<String>,
    },
    Unknown {
        cmd: String,
    },
}
