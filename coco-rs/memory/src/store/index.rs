//! `MEMORY.md` index — line-and-byte truncation, parse pointer entries.
//!
//! The model owns this file; the runtime never auto-regenerates it.
//! We only read + truncate.

/// Hard line cap on `MEMORY.md`. Entries beyond this are dropped with a
/// trailing warning.
pub const MAX_ENTRYPOINT_LINES: usize = 200;

/// Hard byte cap on `MEMORY.md`. Catches long-line indexes that slip
/// past the line cap (p100 was 197KB under 200 lines).
pub const MAX_ENTRYPOINT_BYTES: usize = 25_000;

/// Parsed `MEMORY.md` index — model-curated pointers to memory files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryIndex {
    pub entries: Vec<MemoryIndexEntry>,
}

/// One line of `MEMORY.md`: `- [Title](file.md) — one-line hook`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryIndexEntry {
    pub title: String,
    pub file: String,
    pub hook: String,
}

/// Truncation outcome: the truncated content plus stats so callers can
/// log which cap fired (line vs byte vs both).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrypointTruncation {
    pub content: String,
    pub line_count: usize,
    pub byte_count: usize,
    pub line_truncated: bool,
    pub byte_truncated: bool,
}

/// Truncate `MEMORY.md` content to the line AND byte caps and append a
/// blockquote `> WARNING:` footer when either cap fires.
///
/// Two-pass: line-truncate first (natural boundary), then byte-truncate
/// at the last `\n` before the cap so we don't cut mid-line. The byte
/// check examines the *original* length so we report accurate stats —
/// otherwise long lines that survived line-truncation would understate.
pub fn truncate_entrypoint_content(raw: &str) -> EntrypointTruncation {
    let trimmed = raw.trim();
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let line_count = lines.len();
    let byte_count = trimmed.len();

    let line_truncated = line_count > MAX_ENTRYPOINT_LINES;
    let byte_truncated = byte_count > MAX_ENTRYPOINT_BYTES;

    if !line_truncated && !byte_truncated {
        return EntrypointTruncation {
            content: trimmed.to_string(),
            line_count,
            byte_count,
            line_truncated,
            byte_truncated,
        };
    }

    let mut truncated: String = if line_truncated {
        lines[..MAX_ENTRYPOINT_LINES].join("\n")
    } else {
        trimmed.to_string()
    };

    if truncated.len() > MAX_ENTRYPOINT_BYTES {
        // Cut at the last newline before the cap so we don't sever a line.
        let cap = MAX_ENTRYPOINT_BYTES;
        let cut = truncated[..cap].rfind('\n').unwrap_or(cap);
        truncated.truncate(cut);
    }

    let reason = match (line_truncated, byte_truncated) {
        (true, false) => format!("{line_count} lines (limit: {MAX_ENTRYPOINT_LINES})"),
        (false, true) => format!(
            "{byte_count} bytes (limit: {MAX_ENTRYPOINT_BYTES}) — index entries are too long"
        ),
        _ => format!("{line_count} lines and {byte_count} bytes"),
    };

    truncated.push_str(&format!(
        "\n\n> WARNING: MEMORY.md is {reason}. Only part of it was loaded. Keep index entries to one line under ~200 chars; move detail into topic files."
    ));

    EntrypointTruncation {
        content: truncated,
        line_count,
        byte_count,
        line_truncated,
        byte_truncated,
    }
}

/// Parse a `MEMORY.md` body into structured pointer entries.
///
/// Lines that don't match `- [Title](file.md) — hook` are silently
/// skipped (headers, blank lines, commentary).
pub fn parse_memory_index(content: &str) -> MemoryIndex {
    let entries = content
        .lines()
        .filter(|l| l.trim_start().starts_with("- ["))
        .filter_map(parse_index_line)
        .collect();
    MemoryIndex { entries }
}

fn parse_index_line(line: &str) -> Option<MemoryIndexEntry> {
    let title_start = line.find('[')? + 1;
    let title_end = line[title_start..].find(']')? + title_start;
    let file_start = line[title_end..].find('(')? + title_end + 1;
    let file_end = line[file_start..].find(')')? + file_start;
    let after_paren = &line[file_end + 1..];
    let hook = after_paren
        .trim_start()
        .trim_start_matches(['—', '-'])
        .trim()
        .to_string();
    Some(MemoryIndexEntry {
        title: line[title_start..title_end].to_string(),
        file: line[file_start..file_end].to_string(),
        hook,
    })
}

#[cfg(test)]
#[path = "index.test.rs"]
mod tests;
