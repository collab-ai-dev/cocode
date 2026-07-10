//! Shared `render_for_model` primitives for shell-family tools.
//!
//! Bash and PowerShell (and any future shell wrapper) build the same
//! model-visible text shape: stripped stdout, optional `<persisted-output>`
//! envelope, optional stderr + abort marker, optional background-info
//! line. The string-shaping pieces don't depend on shell-specific state
//! so they live here, behind one tool-private module, instead of being
//! re-imported across siblings as `super::bash::*`.
//!

/// Strip leading blank-only lines from `s` — drops any
/// contiguous run of whitespace-only lines that includes a terminating
/// newline. The final partial line (no trailing newline) is preserved
/// even if blank.
pub(super) fn strip_leading_blank_lines(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let rel_end = match bytes[idx..].iter().position(|&b| b == b'\n') {
            Some(n) => idx + n,
            None => break,
        };
        let line = &s[idx..rel_end];
        if !line.chars().all(char::is_whitespace) {
            break;
        }
        idx = rel_end + 1;
    }
    &s[idx..]
}

#[cfg(test)]
#[path = "shell_render.test.rs"]
mod tests;
