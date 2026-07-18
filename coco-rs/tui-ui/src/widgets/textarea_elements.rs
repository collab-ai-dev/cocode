//! Atomic inline elements embedded in a [`super::TextArea`].

use std::ops::Range;

use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Stable identifier for an element during the lifetime of one textarea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId(i64);

impl ElementId {
    pub(super) fn new(raw: i64) -> Self {
        Self(raw)
    }

    pub(super) fn raw(self) -> i64 {
        self.0
    }
}

/// Domain-free categories understood by the composer shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementKind {
    Paste,
    Image,
    FileRef,
}

/// Styled single-line content displayed in place of an element's source token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementDisplay {
    text: String,
    style: Style,
}

impl ElementDisplay {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub(super) fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }

    pub(super) fn fitted(&self, width: usize) -> Self {
        if self.width() <= width {
            return self.clone();
        }
        if width == 0 {
            return Self::new(String::new(), self.style);
        }
        let content_width = width.saturating_sub(1);
        let mut text = String::new();
        let mut used = 0usize;
        for grapheme in self.text.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if used + grapheme_width > content_width {
                break;
            }
            text.push_str(grapheme);
            used += grapheme_width;
        }
        text.push('…');
        Self::new(text, self.style)
    }
}

/// An indivisible region of the textarea's source buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextElement {
    pub(super) id: ElementId,
    pub(super) range: Range<usize>,
    pub(super) kind: ElementKind,
    pub(super) display: ElementDisplay,
}

impl TextElement {
    pub fn id(&self) -> ElementId {
        self.id
    }

    pub fn range(&self) -> &Range<usize> {
        &self.range
    }

    pub fn kind(&self) -> ElementKind {
        self.kind
    }

    pub fn display(&self) -> &ElementDisplay {
        &self.display
    }

    pub(super) fn display_width(&self) -> usize {
        self.display.width()
    }
}

/// Element metadata after projecting source text into display text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedTextElement {
    pub(super) range: Range<usize>,
    pub(super) display: ElementDisplay,
}

impl ProjectedTextElement {
    pub fn range(&self) -> &Range<usize> {
        &self.range
    }

    pub fn display(&self) -> &ElementDisplay {
        &self.display
    }
}

/// Display text plus the styled ranges that replace source elements.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextProjection {
    pub(super) text: String,
    pub(super) elements: Vec<ProjectedTextElement>,
}

impl TextProjection {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn elements(&self) -> &[ProjectedTextElement] {
        &self.elements
    }

    pub fn into_parts(self) -> (String, Vec<ProjectedTextElement>) {
        (self.text, self.elements)
    }
}

/// Validated wholesale textarea state used by app-level composer snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextAreaSnapshot {
    pub(super) text: String,
    pub(super) cursor: usize,
    pub(super) elements: Vec<TextElement>,
}

impl TextAreaSnapshot {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn elements(&self) -> &[TextElement] {
        &self.elements
    }

    pub fn with_cursor(mut self, cursor: usize) -> Result<Self, super::ElementError> {
        if cursor > self.text.len()
            || !self.text.is_char_boundary(cursor)
            || self
                .elements
                .iter()
                .any(|element| cursor > element.range.start && cursor < element.range.end)
        {
            return Err(super::ElementError::InvalidCursor);
        }
        self.cursor = cursor;
        Ok(self)
    }
}
