//! `COCO_SHELL_PREFIX` formatting.
//!
//! Wraps a command with a user-supplied shell prefix. The prefix may include
//! its own arguments — anything before the last ` -` is treated as the
//! executable path (and quoted as one unit), anything from `-` onward is
//! passed through verbatim. Examples:
//!
//! - `bash`                        → `'bash' 'cmd'`
//! - `/usr/bin/bash -c`            → `'/usr/bin/bash' -c 'cmd'`
//! - `C:\Program Files\Git\bash.exe -c` → `'C:\Program Files\Git\bash.exe' -c 'cmd'`

use crate::shell_quoting::single_quote_for_eval;

/// Format the user-supplied shell prefix + command into a single
/// runnable string.
pub fn format_shell_prefix_command(prefix: &str, command: &str) -> String {
    if let Some(idx) = prefix.rfind(" -") {
        // Guard against `prefix == " -foo"` (idx == 0).
        if idx > 0 {
            let exec_path = &prefix[..idx];
            let args = &prefix[idx + 1..]; // skip the leading space
            return format!(
                "{} {} {}",
                single_quote_for_eval(exec_path),
                args,
                single_quote_for_eval(command),
            );
        }
    }
    format!(
        "{} {}",
        single_quote_for_eval(prefix),
        single_quote_for_eval(command),
    )
}

#[cfg(test)]
#[path = "shell_prefix.test.rs"]
mod tests;
