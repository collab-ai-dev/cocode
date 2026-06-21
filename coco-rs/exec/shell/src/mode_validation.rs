//! Mode-based command validation.
//!
//! In acceptEdits mode, auto-allow certain pure-create file-manipulation
//! commands without requiring user approval.

/// Commands that are auto-allowed in acceptEdits mode.
///
/// Only pure-create commands. `rm`/`rmdir`/`mv`/`cp`/`sed` are intentionally
/// NOT here — they route through the dangerous-removal and sed-danger gates in
/// [`BashTool::check_permissions`](../../../core/tools), which run BEFORE this
/// acceptEdits auto-allow so `rm -rf /` and code-executing `sed` still prompt
/// (a benign `rm foo.txt` / `sed -i s/a/b/ f` falls through to the mode allow).
const ACCEPT_EDITS_COMMANDS: &[&str] = &["mkdir", "touch"];

/// Check if a command should be auto-allowed in acceptEdits mode.
///
/// Returns true if ANY subcommand's base executable is in the auto-allow list.
/// Splits the command and allows if any part is a pure-create command, so
/// `cd src && mkdir out` finds `mkdir` → allow.
pub fn is_auto_allowed_in_accept_edits(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    crate::bash_permissions::split_compound_command(trimmed)
        .iter()
        .any(|sub| ACCEPT_EDITS_COMMANDS.contains(&extract_base_executable(sub.trim())))
}

/// Extract the base executable name from a command string.
/// Strips path prefixes and env var assignments.
pub(crate) fn extract_base_executable(command: &str) -> &str {
    let mut rest = command;

    // Skip env var assignments (VAR=value ...)
    loop {
        let first_word = rest.split_whitespace().next().unwrap_or("");
        if first_word.contains('=') && !first_word.starts_with('=') {
            // Skip past this env assignment
            rest = rest[first_word.len()..].trim_start();
        } else {
            break;
        }
    }

    // Get the first word and strip path
    let cmd = rest.split_whitespace().next().unwrap_or("");
    cmd.rsplit('/').next().unwrap_or(cmd)
}

#[cfg(test)]
#[path = "mode_validation.test.rs"]
mod tests;
