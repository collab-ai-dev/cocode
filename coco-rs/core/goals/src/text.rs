//! UTF-8-safe bounded text used for durable, model-visible goal fields.
//!
//! Every free-form string that reaches the snapshot (objective, progress
//! summary, blocker rationale, …) is length-capped at construction so a pasted
//! specification cannot bloat the append-only session log or a re-injected
//! prompt suffix.

use serde::{Deserialize, Serialize};

/// Default byte budget for a short bounded field (summaries, reasons).
pub const SHORT_TEXT_BUDGET: usize = 2_000;

/// Byte budget for the immutable original objective.
pub const OBJECTIVE_BUDGET: usize = 8_000;

/// A string capped to a byte budget at a UTF-8 char boundary. Never panics on
/// multi-byte content, unlike a raw `&s[..n]` cut.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoundedText(String);

impl BoundedText {
    /// Cap `value` to `max_bytes` at a char boundary. Trailing whitespace left by
    /// the cut is trimmed so the stored form stays tidy.
    pub fn new(value: impl AsRef<str>, max_bytes: usize) -> Self {
        let value = value.as_ref();
        let capped = coco_utils_string::take_bytes_at_char_boundary(value, max_bytes);
        Self(capped.trim_end().to_string())
    }

    /// Cap to [`SHORT_TEXT_BUDGET`].
    pub fn short(value: impl AsRef<str>) -> Self {
        Self::new(value, SHORT_TEXT_BUDGET)
    }

    /// Cap to [`OBJECTIVE_BUDGET`].
    pub fn objective(value: impl AsRef<str>) -> Self {
        Self::new(value, OBJECTIVE_BUDGET)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for BoundedText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[path = "text.test.rs"]
mod tests;
