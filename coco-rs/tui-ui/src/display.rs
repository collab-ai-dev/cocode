//! Render-time display toggles that are config-free (the loader that derives
//! them from `settings.json` lives in the shell).

/// How much language-level syntax highlighting is applied inside fenced code
/// blocks. Diff add/remove colors and other semantic highlights are separate.
///
/// The tier gates which grammars syntect is allowed to compile — the dominant
/// resident-memory cost (see `coco-tui-markdown` `highlight.rs`). `Lite` caps
/// that cost at the startup-prewarm baseline; `Full` lets any grammar compile
/// lazily on first use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SyntaxHighlighting {
    /// No fenced-code highlighting; every code block renders as plain text.
    Off,
    /// Only the prewarmed hot-path grammars (diff, bash) highlight; all other
    /// languages fall back to plain text.
    #[default]
    Lite,
    /// All bundled grammars highlight, compiled lazily on first use.
    Full,
}

impl SyntaxHighlighting {
    /// `true` when no highlighting is applied at all.
    pub fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }

    /// `true` when any highlighting is applied (`Lite` or `Full`).
    pub fn is_enabled(self) -> bool {
        !self.is_off()
    }

    /// Cycle forward `Off → Lite → Full → Off`, matching the ctrl+t binding
    /// in the theme picker.
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Lite,
            Self::Lite => Self::Full,
            Self::Full => Self::Off,
        }
    }
}
