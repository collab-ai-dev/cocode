//! Render-time display toggles that are config-free (the loader that derives
//! them from `settings.json` lives in the shell).

/// Whether language-level syntax highlighting is applied inside fenced code
/// blocks. Diff add/remove colors and other semantic highlights are separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyntaxHighlighting {
    #[default]
    Enabled,
    Disabled,
}

impl SyntaxHighlighting {
    pub fn from_disabled(disabled: bool) -> Self {
        if disabled {
            Self::Disabled
        } else {
            Self::Enabled
        }
    }

    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    pub fn is_disabled(self) -> bool {
        matches!(self, Self::Disabled)
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Enabled,
        }
    }
}
