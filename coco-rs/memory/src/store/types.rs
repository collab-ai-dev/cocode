//! Memory entry data types.
//!
//! The four-type taxonomy is a closed set; content that falls outside
//! it is rejected by `MemoryEntryType::parse`.

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// File name of the per-directory memory index. Always loaded into the
/// model's context.
pub const ENTRYPOINT_NAME: &str = "MEMORY.md";

/// One of the four memory taxonomy variants. The set is closed —
/// content outside it cannot be saved as a memory.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryType {
    /// Role, preferences, knowledge → tailor behavior.
    User,
    /// Corrections AND confirmations → avoid repeating mistakes.
    Feedback,
    /// Goals, decisions, deadlines → understand context.
    Project,
    /// Pointers to external systems → know where to look.
    Reference,
}

impl MemoryEntryType {
    /// Wire-string for serde + manifest formatting.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    /// Accept the four canonical strings, reject anything else. Returns
    /// `None` for malformed / missing types so the caller can decide
    /// whether to skip the file or coerce to `User`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// Frontmatter metadata parsed from a memory file's YAML header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFrontmatter {
    pub name: String,
    pub description: String,
    pub memory_type: MemoryEntryType,
}

/// A loaded memory entry: frontmatter + body + on-disk path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub name: String,
    pub description: String,
    pub memory_type: MemoryEntryType,
    /// Body of the markdown file, frontmatter stripped.
    pub content: String,
    /// Filename relative to the directory the entry was scanned from.
    /// We never store an absolute path here — the caller owns the
    /// directory and joins as needed.
    pub filename: String,
    /// Absolute path to the file on disk. Carried alongside `filename`
    /// so callers that want to re-read or stat the file don't have to
    /// re-resolve. Set by the loader; not part of the wire format.
    pub file_path: PathBuf,
}

#[cfg(test)]
#[path = "types.test.rs"]
mod tests;
