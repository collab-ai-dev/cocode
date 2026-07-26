//! Terminal window/tab title output (OSC 2), and the sanitization that has to
//! happen first.
//!
//! The title is assembled from text coco does not control — model output, a
//! generated session title, a project path. That text is about to be placed
//! *inside* an escape sequence, so a stray `BEL`/`ESC` in it would terminate the
//! sequence early and leave the remainder to be interpreted as terminal
//! commands, and a bidi override would let a title read as something other than
//! what it is (the Trojan Source family). Both are stripped here rather than at
//! the call sites, so there is exactly one place to audit.
//!
//! Restoring the terminal's previous title is deliberately not attempted: there
//! is no portable way to read it back, and the widely-copied "save/restore title
//! stack" escapes are ignored by enough terminals that relying on them produces
//! a title stuck on a finished session. Clearing what coco itself wrote is the
//! honest maximum.

use std::fmt;

/// Practical upper bound on title length in `char`s. Terminals silently
/// truncate long titles; cutting first keeps the tail (usually the most
/// specific part) from being what the terminal drops.
const MAX_TITLE_CHARS: usize = 240;

/// What a [`set_terminal_title`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetTerminalTitleResult {
    /// The sanitized title was written.
    Applied,
    /// Sanitization left nothing visible; nothing was written. Callers should
    /// treat this as "clear the managed title" rather than writing an empty
    /// string, so a title made entirely of stripped characters cannot blank a
    /// tab label into an unlabelled one.
    NoVisibleContent,
}

/// OSC 2 (set window title), ST-terminated.
///
/// A `crossterm::Command` so the write goes through the same `execute!` path as
/// every other escape coco emits.
struct SetTitle<'a>(&'a str);

impl crossterm::Command for SetTitle<'_> {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b]2;{}\x1b\\", self.0)
    }
}

/// Set the terminal title to `title`, after sanitization.
///
/// Best-effort by contract: a non-tty destination is a silent no-op, because a
/// piped or captured stdout must not receive escape bytes.
pub fn set_terminal_title(title: &str) -> std::io::Result<SetTerminalTitleResult> {
    let sanitized = sanitize_title(title);
    if sanitized.is_empty() {
        return Ok(SetTerminalTitleResult::NoVisibleContent);
    }
    write_title(&sanitized)?;
    Ok(SetTerminalTitleResult::Applied)
}

/// Clear the title coco wrote. Does not restore whatever was there before —
/// see the module docs.
pub fn clear_terminal_title() -> std::io::Result<()> {
    write_title("")
}

fn write_title(title: &str) -> std::io::Result<()> {
    use std::io::IsTerminal;

    let mut stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return Ok(());
    }
    crossterm::execute!(stdout, SetTitle(title))
}

/// Strip everything that could escape the OSC sequence or misrepresent its
/// content, then collapse whitespace and cap the length.
///
/// Removed: C0/C1 controls (including the `BEL` and `ESC` that terminate an OSC
/// string), bidi overrides and isolates, and the invisible formatting
/// codepoints — zero-width space/joiner and the word joiner — that let two
/// different titles render identically.
pub fn sanitize_title(title: &str) -> String {
    let stripped: String = title
        .chars()
        .map(|ch| if is_title_hostile(ch) { ' ' } else { ch })
        .collect();
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_TITLE_CHARS {
        return collapsed;
    }
    collapsed.chars().take(MAX_TITLE_CHARS).collect()
}

fn is_title_hostile(ch: char) -> bool {
    // Controls: C0 (incl. BEL/ESC), DEL, and the C1 block.
    if ch.is_control() || ('\u{80}'..='\u{9f}').contains(&ch) {
        return true;
    }
    matches!(
        ch,
        // Bidi embedding/override/isolate controls (Trojan Source).
        '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        // Invisible formatting: ZWSP, ZWNJ, ZWJ, word joiner, BOM/ZWNBSP.
        | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}'
    )
}

#[cfg(test)]
#[path = "terminal_title.test.rs"]
mod tests;
