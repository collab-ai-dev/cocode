//! Shell snapshot capture and restoration.
//!
//! Captures a user's shell environment (aliases, functions, exports, options)
//! and restores it before each command execution. This ensures commands run
//! with the same environment as the user's interactive shell without the
//! overhead of a login shell for every invocation.
//!
//! ## Supported Shells
//!
//! - **zsh**: Full support (functions, setopt, aliases, exports)
//! - **bash**: Full support (functions via base64, shopt/set, aliases, exports)
//! - **sh**: Basic support (functions if available, aliases, exports)
//! - **PowerShell**: Limited support
//! - **cmd**: Not supported

mod cleanup;
mod scripts;
mod shell_snapshot;

pub use cleanup::cleanup_stale_snapshots;
pub use scripts::EXCLUDED_EXPORT_VARS;
pub use scripts::bash_snapshot_script;
pub use scripts::powershell_snapshot_script;
pub use scripts::sh_snapshot_script;
pub use scripts::zsh_snapshot_script;
pub use shell_snapshot::ShellSnapshot;
pub use shell_snapshot::SnapshotConfig;
