use pretty_assertions::assert_eq;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use uuid::Uuid;

use super::*;
use crate::theme::Theme;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::test_helpers;
use crate::transcript::render::HistoryLineRenderOptions;
use crate::transcript::render::HistoryReplayCachePolicy;
use crate::transcript::render::render_finalized_history_lines;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::engine::terminal::SurfaceTerminal;
use coco_tui_ui::style::UiStyles;

/// Build a deterministic UUID from an integer index so tests can name
/// stable cells without keeping local Uuid bindings.
fn uuid_n(n: u32) -> Uuid {
    Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFF_FFFF_0000_0000 | u128::from(n))
}

/// Build a Vec<RenderedCell> with stable UUIDs from a list of indices.
fn cells_with_indices<const N: usize>(indices: [u32; N]) -> Vec<RenderedCell> {
    indices
        .into_iter()
        .map(|i| test_helpers::with_uuid(test_helpers::assistant_text_cell("text"), uuid_n(i)))
        .collect()
}

#[test]
fn plan_appends_all_messages_for_fresh_tracker() {
    let cells = cells_with_indices([1, 2]);
    let tracker = HistoryEmissionTracker::new();

    assert_eq!(
        tracker.plan(&cells),
        HistoryEmissionPlan::Append { start: 0 }
    );
}

#[test]
fn plan_noops_when_emitted_prefix_matches_all_messages() {
    let cells = cells_with_indices([1, 2]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&cells, cells.len());

    assert_eq!(tracker.plan(&cells), HistoryEmissionPlan::Noop);
    assert_eq!(tracker.emitted_count(), 2);
}

#[test]
fn plan_appends_only_new_tail_when_prefix_matches() {
    let initial = cells_with_indices([1]);
    let next = cells_with_indices([1, 2, 3]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&initial, initial.len());

    assert_eq!(
        tracker.plan(&next),
        HistoryEmissionPlan::Append { start: 1 }
    );
}

#[test]
fn mark_appended_from_extends_tracker_without_rebuilding_prefix() {
    let initial = cells_with_indices([1, 2]);
    let next = cells_with_indices([1, 2, 3, 4]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&initial, initial.len());

    tracker.mark_appended_from(&next, 2);

    assert_eq!(tracker.emitted_count(), 4);
    assert_eq!(tracker.plan(&next), HistoryEmissionPlan::Noop);
}

#[test]
fn plan_requires_replay_after_rewind_or_truncate() {
    let original = cells_with_indices([1, 2]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&original, original.len());

    assert_eq!(
        tracker.plan(&cells_with_indices([1])),
        HistoryEmissionPlan::ReplayRequired
    );
}

#[test]
fn plan_requires_replay_after_prefix_divergence() {
    let original = cells_with_indices([1, 2]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&original, original.len());

    assert_eq!(
        tracker.plan(&cells_with_indices([1, 99])),
        HistoryEmissionPlan::ReplayRequired
    );
}

#[test]
fn reset_returns_tracker_to_fresh_append_state() {
    let cells = cells_with_indices([1]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&cells, cells.len());
    tracker.reset();

    assert_eq!(
        tracker.plan(&cells),
        HistoryEmissionPlan::Append { start: 0 }
    );
}

#[test]
fn emit_append_only_writes_new_tail_and_marks_messages() {
    let backend = TestBackend::with_lines(["old   ", "view  "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 6, 1));
    let cells = cells_with_indices([1]);
    let mut tracker = HistoryEmissionTracker::new();

    let outcome = tracker
        .emit_append_only(&mut terminal, &cells, render_cell_ids)
        .expect("emit");

    assert_eq!(
        outcome,
        HistoryEmissionOutcome::Appended {
            start: 0,
            message_count: 1,
            // The 36-char UUID line wraps to 6 rows at width 6 — committed
            // scrollback now wraps like the live tail instead of clipping to one
            // row (which had silently truncated the line).
            rows: 6,
        }
    );
    assert_eq!(tracker.emitted_count(), 1);
}

#[test]
fn emit_append_only_noops_when_already_emitted() {
    let backend = TestBackend::with_lines(["old   ", "view  "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 6, 1));
    let cells = cells_with_indices([1]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&cells, cells.len());

    let outcome = tracker
        .emit_append_only(&mut terminal, &cells, render_cell_ids)
        .expect("emit");

    assert_eq!(outcome, HistoryEmissionOutcome::Noop);
    terminal.backend().assert_buffer_lines(["old   ", "view  "]);
}

#[test]
fn emit_append_only_returns_replay_required_without_touching_terminal() {
    let backend = TestBackend::with_lines(["old   ", "view  "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 6, 1));
    let original = cells_with_indices([1]);
    let mut tracker = HistoryEmissionTracker::new();
    tracker.mark_emitted_through(&original, original.len());

    let outcome = tracker
        .emit_append_only(&mut terminal, &cells_with_indices([99]), render_cell_ids)
        .expect("emit");

    assert_eq!(outcome, HistoryEmissionOutcome::ReplayRequired);
    terminal.backend().assert_buffer_lines(["old   ", "view  "]);
}

#[test]
fn replay_all_clears_surface_reinserts_all_rows_and_marks_messages() {
    let backend = TestBackend::with_lines(["old0  ", "old1  ", "view  "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 2, 6, 1));
    terminal.note_history_rows_inserted(2);
    let cells = cells_with_indices([1, 2]);
    let mut tracker = HistoryEmissionTracker::new();

    let outcome = tracker
        .replay_all(&mut terminal, &cells, render_cell_ids)
        .expect("replay");

    assert_eq!(
        outcome,
        HistoryEmissionOutcome::Replayed {
            message_count: 2,
            // 2 UUID lines × 6 wrapped rows each at width 6 (was clipped to 2).
            rows: 12,
        }
    );
    assert_eq!(tracker.emitted_count(), 2);
    assert_eq!(terminal.visible_history_rows(), 2);
}

#[test]
fn emit_append_only_accepts_finalized_transcript_renderer() {
    let theme = Theme::default();
    let backend = TestBackend::with_lines(["old0    ", "old1    ", "old2    ", "view    "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 3, 8, 1));
    let cells = vec![test_helpers::assistant_text_cell("hello")];
    let mut tracker = HistoryEmissionTracker::new();

    let outcome = tracker
        .emit_append_only(&mut terminal, &cells, |tail| {
            render_finalized_history_lines(
                tail,
                HistoryLineRenderOptions {
                    styles: UiStyles::new(&theme),
                    width: 8,
                    syntax_highlighting: SyntaxHighlighting::Disabled,
                    show_system_reminders: false,
                    show_thinking: false,
                    cwd: None,
                    kb_handle: None,
                    replay_cache_policy: HistoryReplayCachePolicy::default(),
                    reasoning_metadata: None,
                    subagent_summaries: None,
                },
            )
        })
        .expect("emit");

    assert_eq!(
        outcome,
        HistoryEmissionOutcome::Appended {
            start: 0,
            message_count: 1,
            rows: 2,
        }
    );
    assert_eq!(
        plain_buffer_lines(terminal.backend().buffer()),
        vec!["old2    ", "⏺ hello ", "        ", "view    "]
    );
}

fn render_cell_ids(cells: &[RenderedCell]) -> Vec<Line<'static>> {
    cells
        .iter()
        .map(|cell| Line::from(cell.message_uuid.to_string()))
        .collect()
}

fn plain_buffer_lines(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}
