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
use crate::state::session::ReasoningMetadata;
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

/// Render one frame through the real widget, reusing `layout` so the height
/// cache carries across calls exactly as it does between frames.
fn render_with_layout(
    state: &AppState,
    transcript: &TranscriptState,
    layout: &mut TranscriptLayoutIndex,
) {
    let theme = Theme::default();
    let area = Rect::new(0, 0, 60, 20);
    let mut buffer = Buffer::empty(area);
    TranscriptStateWidget::new(state, transcript, layout, UiStyles::new(&theme))
        .render(area, &mut buffer);
}

#[test]
fn test_height_cache_survives_a_generation_bump_from_an_append() {
    // B3. The generation hash moves on ANY transcript or tool-status change,
    // and the cache used to be flushed wholesale for it — so with the reader
    // open during a turn, `total_height()` re-rendered every cell in the
    // history on every change: O(history) full cell renders per delta.
    //
    // Cells are append-only pure derivations (I-2), so an append cannot change
    // an already-measured cell's height. Those entries must survive.
    let _locale = locale_test_guard("en");
    let mut state = seeded_app_state();
    let transcript = TranscriptState::new();
    let mut layout = TranscriptLayoutIndex::default();

    render_with_layout(&state, &transcript, &mut layout);
    let measured: Vec<_> = layout
        .heights
        .iter()
        .map(|(key, height)| (key.clone(), *height))
        .collect();
    assert!(
        !measured.is_empty(),
        "the first render must measure and cache cell heights"
    );
    let generation_before = layout.content_generation;

    push_cells(&mut state, [assistant_text_cell("One more reply.")]);
    render_with_layout(&state, &transcript, &mut layout);

    assert_ne!(
        layout.content_generation, generation_before,
        "an append must bump the content generation (else this test proves nothing)"
    );
    for (key, height) in measured {
        assert_eq!(
            layout.heights.get(&key),
            Some(&height),
            "an append must not discard an already-measured cell's height: {key:?}"
        );
    }
}

#[test]
fn test_height_cache_key_separates_tool_execution_status() {
    // The one non-cell input to a cell's height: a tool cell's render reads
    // live `ToolExecution` state (UI-only, I-3). Now that the map survives a
    // generation bump, status must be part of the key — otherwise a completed
    // tool would be served the height it had while running.
    let _locale = locale_test_guard("en");
    let mut state = seeded_app_state();
    state.session.start_tool(
        "call-1".to_string(),
        "Grep".to_string(),
        &serde_json::json!({"pattern": "fn main"}),
    );
    let transcript = TranscriptState::new();
    let mut layout = TranscriptLayoutIndex::default();

    render_with_layout(&state, &transcript, &mut layout);
    let running: Vec<_> = layout
        .heights
        .keys()
        .filter(|key| key.tool_layout.is_some())
        .cloned()
        .collect();
    assert!(
        !running.is_empty(),
        "the tool cell's key must carry its execution status"
    );

    // Complete the tool: same cell id, different live status.
    state.session.complete_tool("call-1", /*failed*/ false);
    render_with_layout(&state, &transcript, &mut layout);

    for key in running {
        let completed = layout.heights.keys().any(|other| {
            other.cell_id == key.cell_id
                && other.width == key.width
                && other.expanded == key.expanded
                && other.tool_layout != key.tool_layout
        });
        assert!(
            completed,
            "a status change must re-measure under a new key rather than reuse {key:?}"
        );
    }
}

#[test]
fn test_height_cache_key_tracks_elapsed_badge_width() {
    let _locale = locale_test_guard("en");
    let mut state = seeded_app_state();
    state.session.start_tool(
        "call-1".to_string(),
        "Grep".to_string(),
        &serde_json::json!({"pattern": "fn main"}),
    );
    let now = std::time::Instant::now();
    let execution = state
        .session
        .tool_executions
        .iter_mut()
        .find(|tool| tool.call_id == "call-1")
        .expect("running tool");
    execution.started_at = now - std::time::Duration::from_secs(1);
    execution.completed_at = Some(now);

    let transcript = TranscriptState::new();
    let mut layout = TranscriptLayoutIndex::default();
    render_with_layout(&state, &transcript, &mut layout);
    let short = layout
        .heights
        .keys()
        .find(|key| key.tool_layout.is_some())
        .cloned()
        .expect("tool cache key");

    let execution = state
        .session
        .tool_executions
        .iter_mut()
        .find(|tool| tool.call_id == "call-1")
        .expect("running tool");
    execution.started_at = now - std::time::Duration::from_secs(100);
    render_with_layout(&state, &transcript, &mut layout);

    assert!(layout.heights.keys().any(|key| {
        key.cell_id == short.cell_id
            && key.tool_layout.map(|(_, width)| width) != short.tool_layout.map(|(_, width)| width)
    }));
}

#[test]
fn test_height_cache_key_tracks_reasoning_metadata() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    let thinking = crate::transcript::derive::test_helpers::assistant_thinking_cell("planning");
    let uuid = thinking.message_uuid;
    push_cells(&mut state, [thinking]);
    state.session.reasoning_metadata.insert(
        uuid,
        ReasoningMetadata {
            duration_ms: Some(900),
            reasoning_tokens: 10,
        },
    );
    let transcript = TranscriptState::new();
    let mut layout = TranscriptLayoutIndex::default();
    render_with_layout(&state, &transcript, &mut layout);
    let before = layout
        .heights
        .keys()
        .find(|key| key.reasoning_metadata.is_some())
        .cloned()
        .expect("reasoning cache key");

    state.session.reasoning_metadata.insert(
        uuid,
        ReasoningMetadata {
            duration_ms: Some(1_300),
            reasoning_tokens: 15,
        },
    );
    render_with_layout(&state, &transcript, &mut layout);

    assert!(layout.heights.keys().any(|key| {
        key.cell_id == before.cell_id && key.reasoning_metadata != before.reasoning_metadata
    }));
}

#[test]
fn test_height_cache_is_bounded_across_a_truncation() {
    // Entries whose cells a truncation removed are unreachable rather than
    // wrong (their ids are gone), but they must not accumulate forever.
    let _locale = locale_test_guard("en");
    let mut state = seeded_app_state();
    let transcript = TranscriptState::new();
    let mut layout = TranscriptLayoutIndex::default();
    render_with_layout(&state, &transcript, &mut layout);

    for i in 0..400 {
        push_cells(&mut state, [assistant_text_cell(&format!("reply {i}"))]);
        state.session.transcript.on_message_truncated(5);
        render_with_layout(&state, &transcript, &mut layout);
    }

    let cells = state.session.transcript.cells().len();
    assert!(
        layout.heights.len() <= (cells * 4).max(super::MIN_RETAINED_HEIGHTS),
        "the retained height map must stay bounded, got {} entries for {cells} cells",
        layout.heights.len()
    );
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
