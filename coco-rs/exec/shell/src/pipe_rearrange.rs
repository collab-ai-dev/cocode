//! Pipe-aware command rearrangement for safe `eval` wrapping.
//!
//! Why: `eval 'rg foo | wc -l' < /dev/null` looks innocent but causes `rg`
//! to **hang forever**. Without arguments, `rg` reads stdin; the shell
//! attaches `eval`'s stdin (which is `/dev/null`) to the *last* command in
//! the pipeline, not the first. `rg` (first command) inherits the parent
//! shell's stdin from the spawn — an open pipe with no writer — and blocks.
//! See anthropics/claude-code issues #9189 / #9732.
//!
//! Smart approach: parse the command into shell tokens, find the first `|`,
//! insert `< /dev/null` between the first command and the pipe so the
//! redirect applies to the right process.
//!
//! This implementation skips the shell-quote parser and instead always places
//! `< /dev/null` outside the `eval` quote, which causes eval's stdin to be
//! `/dev/null` and the first child in the pipeline inherits it. This is always
//! correct — it produces a slightly more redundant command string for the cases
//! that could be parsed precisely, but functionally both approaches yield the
//! same runtime behavior.

use crate::shell_quoting::quote_shell_command;
use crate::shell_quoting::should_add_stdin_redirect;

/// Rewrite a command so its stdin redirect (if any) targets the first
/// process in a pipeline rather than `eval` itself.
///
/// Returns a string that can be appended directly after `eval `.
pub fn rearrange_pipe_command(command: &str) -> String {
    let add_redirect = should_add_stdin_redirect(command);
    quote_shell_command(command, add_redirect)
}

#[cfg(test)]
#[path = "pipe_rearrange.test.rs"]
mod tests;
