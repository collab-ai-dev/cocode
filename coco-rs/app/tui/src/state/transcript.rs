//! Transcript reader state.
//!
//! This module contains logical interaction state only. Render/layout
//! measurement caches live in the surface renderer, outside `AppState`.

use std::collections::HashSet;

/// Transcript state — cell-level reader for `Ctrl+O`.
#[derive(Debug, Clone, Default)]
pub struct TranscriptState {
    /// Logical scroll intent. The renderer resolves it against the current
    /// layout without writing derived metrics back into state.
    pub(crate) scroll: TranscriptScrollPosition,
    /// Expandable cell currently selected for actions such as collapse/expand.
    pub(crate) selected_cell_id: Option<TranscriptCellId>,
    /// Cell ids explicitly collapsed in this state session only.
    ///
    /// Transcript opens expanded by default; this set records opt-in
    /// collapses instead of opt-in expansion.
    pub(crate) collapsed_cell_ids: HashSet<TranscriptCellId>,
}

impl TranscriptState {
    /// Open with default state — scrolled to top with no expanded cells.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::new_with_anchor(None)
    }

    pub(crate) fn new_with_anchor(anchor_cell_id: Option<TranscriptCellId>) -> Self {
        Self {
            scroll: anchor_cell_id
                .clone()
                .map(TranscriptScrollPosition::anchor)
                .unwrap_or_default(),
            selected_cell_id: anchor_cell_id,
            collapsed_cell_ids: HashSet::new(),
        }
    }
}

/// Logical transcript scroll position.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum TranscriptScrollPosition {
    /// Absolute row offset from the top.
    #[default]
    Top,
    /// Absolute row offset from the top.
    Absolute(usize),
    /// Keep a specific cell near the top, with a signed row offset.
    Anchor {
        cell_id: TranscriptCellId,
        offset_rows: i32,
    },
    /// Keep the viewport pinned to the bottom, optionally scrolled upward.
    Tail { offset_from_bottom: usize },
}

impl TranscriptScrollPosition {
    const ANCHOR_CONTEXT_ROWS: i32 = -2;

    pub(crate) fn anchor(cell_id: TranscriptCellId) -> Self {
        Self::Anchor {
            cell_id,
            offset_rows: Self::ANCHOR_CONTEXT_ROWS,
        }
    }

    pub(crate) fn scroll_lines(&mut self, delta: i32) {
        match self {
            Self::Top => {
                if delta > 0 {
                    *self = Self::Absolute(delta as usize);
                }
            }
            Self::Absolute(top) => {
                if delta < 0 {
                    *top = top.saturating_sub(delta.unsigned_abs() as usize);
                } else {
                    *top = top.saturating_add(delta as usize);
                }
                if *top == 0 {
                    *self = Self::Top;
                }
            }
            Self::Anchor { offset_rows, .. } => {
                *offset_rows = offset_rows.saturating_add(delta);
            }
            Self::Tail { offset_from_bottom } => {
                if delta < 0 {
                    *offset_from_bottom =
                        offset_from_bottom.saturating_add(delta.unsigned_abs() as usize);
                } else {
                    *offset_from_bottom = offset_from_bottom.saturating_sub(delta as usize);
                }
            }
        }
    }

    pub(crate) fn jump_start(&mut self) {
        *self = Self::Top;
    }

    pub(crate) fn jump_end(&mut self) {
        *self = Self::Tail {
            offset_from_bottom: 0,
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum TranscriptCellId {
    ToolCall { call_id: String },
    Message { index: usize, message_id: String },
    ToolBatch { start: usize, end: usize },
    ActiveTail,
}

impl TranscriptCellId {
    pub(crate) fn tool(call_id: impl Into<String>) -> Self {
        Self::ToolCall {
            call_id: call_id.into(),
        }
    }

    pub(crate) fn message(index: usize, message_id: impl Into<String>) -> Self {
        Self::Message {
            index,
            message_id: message_id.into(),
        }
    }

    pub(crate) fn tool_batch(start: usize, end: usize) -> Self {
        Self::ToolBatch { start, end }
    }
}
