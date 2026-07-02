//! Whole-pipeline read-only safety check.
//!
//! The shared write-fence primitive for background agents: given a raw
//! `Bash` command string, decide whether it is a pipeline of exclusively
//! read-only (known-safe) stages with no state-mutating shell constructs.

use crate::ShellParser;
use crate::safety::is_known_safe_command;

/// True when `cmd` parses into a sequence of word-only argv stages (chained
/// with safe operators `&&` / `||` / `;` / `|`) where every stage is
/// [`is_known_safe_command`].
///
/// **Fail-closed**: returns `false` when the command is empty, contains a
/// redirection / subshell / command-substitution / syntax error (so
/// `try_extract_safe_commands` yields `None`), or any stage is not known-safe.
/// So `git log --oneline | head -10` is allowed, while `echo x > /etc/passwd`
/// (redirection) and `rm -rf /` (mutating stage) are rejected.
pub fn is_read_only_pipeline(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut parser = ShellParser::new();
    let parsed = parser.parse(trimmed);
    let Some(stages) = parsed.try_extract_safe_commands() else {
        return false;
    };
    if stages.is_empty() {
        return false;
    }
    stages.iter().all(|argv| is_known_safe_command(argv))
}

#[cfg(test)]
#[path = "is_read_only_pipeline.test.rs"]
mod tests;
