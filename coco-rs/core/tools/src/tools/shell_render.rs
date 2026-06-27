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

/// Build the `<persisted-output>` envelope text that replaces stdout
/// in the model-visible result when shell output overflowed the inline
/// budget (`PREVIEW_SIZE_BYTES = 2000`).
///
/// `preview_source` is the already-truncated inline `stdout` field; we
/// take its first 2KB on a UTF-8 char boundary and append `\n...\n`
/// when there is more, otherwise just `\n`.
#[cfg(test)]
pub(super) fn build_persisted_output_message(
    path: &str,
    original_size: usize,
    preview_source: &str,
) -> String {
    const PREVIEW_SIZE_BYTES: usize = 2000;
    let (preview, has_more) = if preview_source.len() > PREVIEW_SIZE_BYTES {
        let mut cut = PREVIEW_SIZE_BYTES.min(preview_source.len());
        while cut > 0 && !preview_source.is_char_boundary(cut) {
            cut -= 1;
        }
        (&preview_source[..cut], true)
    } else {
        (preview_source, false)
    };
    let mut buf = String::with_capacity(preview.len() + 256);
    buf.push_str("<persisted-output>\n");
    buf.push_str(&format!(
        "Output too large ({}). Full output saved to: {path}\n\n",
        format_byte_size(original_size)
    ));
    buf.push_str(&format!(
        "Preview (first {}):\n",
        format_byte_size(PREVIEW_SIZE_BYTES)
    ));
    buf.push_str(preview);
    buf.push_str(if has_more { "\n...\n" } else { "\n" });
    buf.push_str("</persisted-output>");
    buf
}

/// Format a byte count to a human-readable string:
/// - `< 1024`: `"X bytes"` (literal "bytes", no space)
/// - `< 1MB`: `"X.YKB"` (1 decimal, strip trailing `.0`, no space)
/// - `< 1GB`: `"X.YMB"`
/// - otherwise: `"X.YGB"`
#[cfg(test)]
pub(crate) fn format_byte_size(bytes: usize) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{bytes} bytes");
    }
    if kb < 1024.0 {
        return format!("{}KB", trim_trailing_zero_decimal(kb));
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{}MB", trim_trailing_zero_decimal(mb));
    }
    let gb = mb / 1024.0;
    format!("{}GB", trim_trailing_zero_decimal(gb))
}

/// Format a float with 1 decimal, then strip the trailing `.0`.
#[cfg(test)]
fn trim_trailing_zero_decimal(n: f64) -> String {
    let s = format!("{n:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
#[path = "shell_render.test.rs"]
mod tests;
