//! Semantic style accessors over the active theme.

use std::hash::Hash;
use std::hash::Hasher;

use ratatui::style::Color;
use ratatui::style::Style;

use crate::theme::Theme;

#[derive(Debug, Clone, Copy)]
pub struct UiStyles<'a> {
    theme: &'a Theme,
}

impl<'a> UiStyles<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }

    pub fn primary_border(self) -> Style {
        Style::default().fg(self.theme.primary)
    }

    /// Stable fingerprint of the active palette, for render cache keys. Two
    /// `UiStyles` over equal palettes hash identically; any color change (incl.
    /// a hot-reloaded `theme.json`) changes the fingerprint, invalidating caches.
    pub fn theme_hash(self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.theme.hash(&mut hasher);
        hasher.finish()
    }

    pub fn border(self) -> Color {
        self.theme.border
    }

    pub fn focused_border(self) -> Color {
        self.theme.border_focused
    }

    pub fn text(self) -> Color {
        self.theme.text
    }

    pub fn primary(self) -> Color {
        self.theme.primary
    }

    pub fn secondary(self) -> Color {
        self.theme.secondary
    }

    pub fn accent(self) -> Color {
        self.theme.accent
    }

    pub fn dim(self) -> Color {
        self.theme.text_dim
    }

    pub fn success(self) -> Color {
        self.theme.success
    }

    pub fn warning(self) -> Color {
        self.theme.warning
    }

    pub fn error(self) -> Color {
        self.theme.error
    }

    pub fn plan(self) -> Color {
        self.theme.plan_mode
    }

    pub fn thinking(self) -> Color {
        self.theme.thinking
    }

    pub fn tool_running(self) -> Color {
        self.theme.tool_running
    }

    pub fn tool_completed(self) -> Color {
        self.theme.tool_completed
    }

    pub fn tool_error(self) -> Color {
        self.theme.tool_error
    }

    pub fn selection_bg(self) -> Color {
        self.theme.selection_bg
    }

    pub fn selection_fg(self) -> Color {
        self.theme.selection_fg
    }

    pub fn progress_bar(self) -> Color {
        self.theme.progress_bar
    }

    pub fn context_used(self) -> Color {
        self.theme.context_used
    }

    pub fn context_free(self) -> Color {
        self.theme.context_free
    }

    pub fn user_message(self) -> Color {
        self.theme.user_message
    }

    pub fn assistant_message(self) -> Color {
        self.theme.assistant_message
    }

    pub fn user_message_bg(self) -> Option<Color> {
        self.theme.user_message_bg
    }

    pub fn system_message(self) -> Color {
        self.theme.system_message
    }

    pub fn diff_removed(self) -> Color {
        self.theme.diff_removed
    }

    pub fn diff_added(self) -> Color {
        self.theme.diff_added
    }

    pub fn table_border(self) -> Color {
        self.theme.table_border
    }

    pub fn table_header(self) -> Color {
        self.theme.table_header
    }

    pub fn code_comment(self) -> Color {
        self.theme.code_comment
    }

    pub fn code_string(self) -> Color {
        self.theme.code_string
    }

    pub fn code_number(self) -> Color {
        self.theme.code_number
    }

    pub fn code_keyword(self) -> Color {
        self.theme.code_keyword
    }

    pub fn code_function(self) -> Color {
        self.theme.code_function
    }

    pub fn code_type(self) -> Color {
        self.theme.code_type
    }

    pub fn code_operator(self) -> Color {
        self.theme.code_operator
    }

    pub fn code_inline(self) -> Color {
        self.theme.code_inline
    }

    pub fn code_bg(self) -> Option<Color> {
        self.theme.code_bg
    }

    pub fn blockquote(self) -> Color {
        self.theme.blockquote
    }

    pub fn heading(self) -> Color {
        self.theme.heading
    }

    pub fn hr(self) -> Color {
        self.theme.hr
    }

    pub fn strikethrough(self) -> Color {
        self.theme.strikethrough
    }

    pub fn modal_border(self) -> Color {
        self.theme.modal_border
    }

    pub fn panel_border(self) -> Color {
        self.theme.panel_border
    }

    pub fn hyperlink(self) -> Color {
        self.theme.hyperlink
    }
}
