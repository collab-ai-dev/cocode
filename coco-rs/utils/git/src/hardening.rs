//! Git-config hardening for every git process coco spawns on its own behalf.
//!
//! Internal git invocations run automatically inside repositories coco did not
//! author, so repo-local config is attacker-controlled input:
//!
//! - `safe.bareRepository=explicit` — refuses bare repositories found by
//!   upward discovery. A bare repo planted inside a working tree carries its
//!   own `config`/`hooks`, so any git command that discovers it executes
//!   attacker config (anthropics/claude-code#29316 — the same class the shell
//!   executor's post-command scrub defends against). Bare repos passed
//!   explicitly via `GIT_DIR` / `--git-dir` (ghost commits) keep working.
//! - `core.fsmonitor=false` — a repo can set `core.fsmonitor` to an arbitrary
//!   executable that git runs on every status-like command. Internal calls
//!   never need the monitor; the only cost is an uncached scan on very large
//!   repos. User-issued git commands (Bash tool) are unaffected.
//!
//! `-c` overrides must precede the subcommand, so use the constructors (or
//! prepend [`HARDENED_CONFIG_ARGS`]) instead of pushing flags onto an existing
//! argument list.

/// `-c` overrides applied to every internal git invocation.
pub const HARDENED_CONFIG_ARGS: [&str; 4] = [
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.fsmonitor=false",
];

/// A `git` [`std::process::Command`] with the hardening flags pre-applied.
pub fn hardened_std_git() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.args(HARDENED_CONFIG_ARGS);
    cmd
}

/// A `git` [`tokio::process::Command`] with the hardening flags pre-applied.
pub fn hardened_tokio_git() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(HARDENED_CONFIG_ARGS);
    cmd
}

#[cfg(test)]
#[path = "hardening.test.rs"]
mod tests;
