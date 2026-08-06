//! Provider abstraction for shell-specific command building.
//!
//! The provider is the **only** thing that knows about per-shell quirks:
//! snapshot sourcing, session-env injection, extglob disabling, alias
//! expansion, `pwd -P` tracking, base64-encoded PowerShell commands,
//! sandbox `TMPDIR` overrides, `COCO_SHELL_PREFIX` wrapping. The executor
//! (`crate::executor::ShellExecutor`) is just a spawn / wait / cancel /
//! timeout / sandbox-wrap loop on top.
//!
//! Two implementations ship:
//! - [`bash::BashProvider`] for bash / zsh / sh (full pipeline)
//! - [`powershell::PowerShellProvider`] for pwsh / powershell (UTF-16-LE
//!   base64-encoded command path)

pub mod bash;
pub mod powershell;

pub use bash::BashProvider;
pub use powershell::PowerShellProvider;

pub use coco_types::BuildExecOpts;
pub use coco_types::BuiltCommand;
pub use coco_types::ShellProvider;
