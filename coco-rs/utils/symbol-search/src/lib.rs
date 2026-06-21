//! Symbol search for code navigation.
//!
//! Provides tree-sitter-based symbol extraction and fuzzy search
//! for the TUI `@#SymbolName` mention feature.

mod error;
pub mod extractor;
pub mod index;
pub mod languages;
pub mod watcher;

pub use error::SymbolSearchError;
pub use index::SymbolIndex;

/// Kind of symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Type,
    Enum,
    Module,
    Constant,
    Other,
}

impl SymbolKind {
    /// Create from tree-sitter-tags syntax type string.
    pub fn from_syntax_type(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "function" | "func" | "fn" => Self::Function,
            "method" => Self::Method,
            "class" => Self::Class,
            "struct" => Self::Struct,
            "interface" | "trait" => Self::Interface,
            "type" | "typedef" => Self::Type,
            "enum" => Self::Enum,
            "module" | "namespace" | "mod" => Self::Module,
            "constant" | "const" | "variable" | "var" => Self::Constant,
            _ => Self::Other,
        }
    }

    /// Short label for display.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Interface => "trait",
            Self::Type => "type",
            Self::Enum => "enum",
            Self::Module => "mod",
            Self::Constant => "const",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// A symbol search result.
#[derive(Debug, Clone)]
pub struct SymbolSearchResult {
    /// Symbol name (original case).
    pub name: String,
    /// Kind of symbol.
    pub kind: SymbolKind,
    /// File path relative to root.
    pub file_path: String,
    /// Line number (1-indexed).
    pub line: i32,
    /// Fuzzy match score (lower = better).
    pub score: i32,
    /// Character indices that matched the query.
    pub match_indices: Vec<usize>,
}
