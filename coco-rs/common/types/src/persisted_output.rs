//! Canonical `<persisted-output>` reference markers and predicates.
//!
//! Owned here (foundation layer) because both `coco-tool-runtime` (which
//! renders the references) and `coco-compact` (whose micro-compaction must
//! never clear a pointer-bearing result) need the same closed vocabulary —
//! duplicating the literals across crates invites silent drift that destroys
//! the only pointer to offloaded data.

/// Opening tag of a persisted-output reference block.
pub const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
/// Closing tag of a persisted-output reference block.
pub const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

/// Prefix-only predicate: is `content` an already-persisted reference?
///
/// Deliberately distinct from [`is_pointer_bearing`]: this stays `false` for
/// windowed inline output (whose reference footer is a *suffix*) so windowed
/// results remain eligible for Level-2 accounting.
pub fn is_content_already_persisted(content: &str) -> bool {
    content.trim_start().starts_with(PERSISTED_OUTPUT_TAG)
}

/// Prefix-or-suffix predicate: does `content` carry a persisted-output
/// pointer? True for full references (prefix tag) and for windowed inline
/// output (trailing reference footer). Used wherever destroying the content
/// would destroy the only pointer to the offloaded data (micro-compaction,
/// Level-2 eviction eligibility).
pub fn is_pointer_bearing(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.starts_with(PERSISTED_OUTPUT_TAG) || trimmed.ends_with(PERSISTED_OUTPUT_CLOSING_TAG)
}

/// For windowed inline output (suffix reference footer), return the trailing
/// `<persisted-output>…</persisted-output>` block — the minimal
/// pointer-preserving remnant a clearing pass may reduce the content to.
///
/// `None` when there is nothing to strip: plain content (no footer), or a
/// content that IS already just the reference block (prefix form / previously
/// reduced) — callers must leave those intact rather than clearing them.
pub fn pointer_footer(content: &str) -> Option<&str> {
    if !content.trim_end().ends_with(PERSISTED_OUTPUT_CLOSING_TAG) {
        return None;
    }
    // The footer's own opening tag is the LAST occurrence. (The closing tag
    // cannot false-match: `</persisted-output>` does not contain the opening
    // `<persisted-output>` byte sequence.)
    let idx = content.rfind(PERSISTED_OUTPUT_TAG)?;
    if content[..idx].trim().is_empty() {
        return None; // already minimal — nothing to strip
    }
    Some(&content[idx..])
}

#[cfg(test)]
#[path = "persisted_output.test.rs"]
mod tests;
