//! Per-session shell environment that's layered on top of every command.
//!
//! Two ingredients are combined at command-build time:
//!
//! 1. **Hook env files** (`./hook_files.rs`) — shell snippets written by
//!    `SessionStart` / `Setup` / `CwdChanged` / `FileChanged` hooks. Each
//!    snippet is sourced into the shell before the user command, so a hook
//!    can install env vars (e.g. activate a venv) that persist across all
//!    subsequent shell commands in the session.
//!
//! 2. **`/env` store** (`./vars.rs`) — env vars the user set interactively.
//!    Applied as `extra_env` on the spawn, so they're scoped to the child
//!    process — not the coco REPL itself.
//!

mod hook_files;
mod vars;

pub use hook_files::HOOK_ENV_FILE_REGEX;
pub use hook_files::SessionEnvReader;
pub use hook_files::session_env_dir;
pub use vars::SessionEnvVars;
