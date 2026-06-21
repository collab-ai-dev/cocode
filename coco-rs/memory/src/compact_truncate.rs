//! Pure session-memory truncation called by the compact-side glue.
//!
//! Compact (L3) does not depend on memory (L4); the app/query layer
//! owns the wiring and feeds the resulting string to compact as
//! ordinary input. This module is just a stateless transform.

/// Truncate a 9-section session-memory document so the per-section
/// budget is respected.
///
/// `per_section_tokens` is the limit in tokens; we approximate as
/// `per_section_tokens * 4` bytes. Sections are delimited by `# ` at
/// the start of a line. Each section's body (content past its header
/// and italicized hint) is cut at the last newline before the limit
/// and tagged with `[... section truncated for length ...]` if it
/// overflowed.
pub fn truncate_session_memory_for_compact(content: &str, per_section_tokens: i64) -> String {
    let max_bytes = (per_section_tokens.max(1) as usize) * 4;
    let mut out = String::with_capacity(content.len());
    let mut current = String::new();
    let mut header_seen = false;

    for line in content.lines() {
        if line.starts_with("# ") {
            if header_seen {
                flush(&mut out, &current, max_bytes);
                current.clear();
            }
            header_seen = true;
            current.push_str(line);
            current.push('\n');
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if header_seen {
        flush(&mut out, &current, max_bytes);
    } else {
        out.push_str(content);
    }
    out
}

fn flush(out: &mut String, section: &str, max_bytes: usize) {
    if section.len() <= max_bytes {
        out.push_str(section);
        return;
    }
    let cut = section[..max_bytes].rfind('\n').unwrap_or(max_bytes);
    out.push_str(&section[..cut]);
    out.push('\n');
    out.push_str("[... section truncated for length ...]\n");
}

#[cfg(test)]
#[path = "compact_truncate.test.rs"]
mod tests;
