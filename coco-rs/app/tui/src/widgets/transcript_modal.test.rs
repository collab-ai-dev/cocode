//! Companion tests for the Ctrl+O transcript reader widget.
//!
//! The reader is the crate's largest widget and renders the same engine
//! cells as the chat surface but with its own window/selection/collapse
//! pipeline — these snapshots pin that pipeline end to end (cell windowing,
//! tool pairing, selection marker, opt-out collapse) through the real
//! `TranscriptStateWidget::render`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use uuid::Uuid;

use crate::i18n::locale_test_guard;
use crate::state::AppState;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptState;
use crate::theme::Theme;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::test_helpers::assistant_text_cell;
use crate::transcript::derive::test_helpers::info_cell;
use crate::transcript::derive::test_helpers::tool_result_cell;
use crate::transcript::derive::test_helpers::tool_use_cell;
use crate::transcript::derive::test_helpers::user_text_cell;
use crate::widgets::TranscriptLayoutIndex;
use crate::widgets::TranscriptStateWidget;
use coco_tui_ui::style::UiStyles;

fn render_to_text(
    state: &AppState,
    transcript: &TranscriptState,
    width: u16,
    height: u16,
) -> String {
    let theme = Theme::default();
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let mut layout = TranscriptLayoutIndex::default();
    TranscriptStateWidget::new(state, transcript, &mut layout, UiStyles::new(&theme))
        .render(area, &mut buffer);
    buffer
        .content
        .chunks(width as usize)
        .map(|cells| {
            cells
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn push_cells(state: &mut AppState, cells: impl IntoIterator<Item = RenderedCell>) {
    for cell in cells {
        state.session.transcript.on_message_appended(cell.source);
    }
}

fn seeded_app_state() -> AppState {
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            user_text_cell(Uuid::new_v4(), "Please grep the repo"),
            assistant_text_cell("Searching now."),
            tool_use_cell("call-1", "Grep", serde_json::json!({"pattern": "fn main"})),
            tool_result_cell("call-1", "Grep", "src/main.rs:1:fn main() {"),
            info_cell("notice", "Build finished"),
        ],
    );
    app_state
}

#[test]
fn test_reader_renders_mixed_transcript() {
    let _locale = locale_test_guard("en");
    let app_state = seeded_app_state();
    let transcript = TranscriptState::new();
    insta::assert_snapshot!(
        "transcript_modal_mixed_transcript",
        render_to_text(&app_state, &transcript, 60, 16)
    );
}

#[test]
fn test_reader_marks_selected_tool_and_honors_collapse() {
    // Selection marker on the anchored tool cell, with the cell explicitly
    // collapsed (the reader opens expanded by default; `collapsed_cell_ids`
    // records opt-OUT, not opt-in).
    let _locale = locale_test_guard("en");
    let app_state = seeded_app_state();
    let mut transcript = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));
    transcript
        .collapsed_cell_ids
        .insert(TranscriptCellId::tool("call-1"));
    insta::assert_snapshot!(
        "transcript_modal_selected_tool_collapsed",
        render_to_text(&app_state, &transcript, 60, 16)
    );
}

#[test]
fn test_reader_window_survives_short_viewport() {
    // A 6-row window over the same transcript: the reader must render only
    // the visible cells (no panic, no overdraw) and keep the anchored cell
    // in view.
    let _locale = locale_test_guard("en");
    let app_state = seeded_app_state();
    let transcript = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));
    insta::assert_snapshot!(
        "transcript_modal_short_viewport",
        render_to_text(&app_state, &transcript, 60, 6)
    );
}
