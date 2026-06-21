//! Shared sentinel-line parser for slash-command handlers.
//!
//! Slash-command handlers can't drive engine state directly — they
//! return plain strings via the registry. To dispatch a structured
//! request (e.g. fire `/compact`, `/dream`, `/summary`), the handler
//! emits a single first line beginning with a sentinel constant; the
//! TUI / SDK runner detects it, drops it from displayed output, and
//! routes the request to the right subsystem.
//!
//! The shape is uniform: `<SENTINEL>[ <args>]\n<status text>`. Args
//! after the sentinel are optional and trimmed; the remainder of the
//! handler output is the user-visible status block.

/// One parsed sentinel line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSentinel<'a> {
    /// Trimmed text following the sentinel on the first line. Empty
    /// when the handler emitted the bare sentinel (`/dream`, `/summary`).
    pub args: &'a str,
    /// Remaining output after the sentinel newline — typically the
    /// user-visible status text.
    pub status: &'a str,
}

/// Parse a sentinel-prefixed handler output. Returns `None` when the
/// first line doesn't begin with `prefix`.
#[must_use]
pub fn parse_sentinel<'a>(handler_output: &'a str, prefix: &str) -> Option<ParsedSentinel<'a>> {
    let mut lines = handler_output.splitn(2, '\n');
    let first = lines.next()?;
    let rest = lines.next().unwrap_or("");
    let after = first.strip_prefix(prefix)?;
    Some(ParsedSentinel {
        args: after.trim(),
        status: rest,
    })
}

#[cfg(test)]
#[path = "sentinel.test.rs"]
mod tests;
