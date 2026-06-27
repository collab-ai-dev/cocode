use pretty_assertions::assert_eq;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::layout::Size;

use super::*;

// Representative shell policy bounds (the shell owns the real constants).
const MIN_H: u16 = 4;
const MAX_H: u16 = 12;

fn inputs(screen: Size, desired_height: u16) -> SeatInputs {
    SeatInputs {
        screen,
        desired_height,
        min_height: MIN_H,
        max_height: MAX_H,
        guaranteed_append_rows: 0,
    }
}

fn terminal_with_viewport(screen: Size, viewport: Rect) -> SurfaceTerminal<TestBackend> {
    let backend = TestBackend::new(screen.width, screen.height);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(screen);
    terminal.set_viewport_area(viewport);
    terminal
}

// ─── seat_viewport: the engine-owned decision ───────────────────────────────

#[test]
fn seat_flows_after_history_before_screen_fills() {
    let screen = Size::new(80, 24);
    let terminal = terminal_with_viewport(screen, Rect::new(0, 3, 80, 6));

    let decision = terminal.seat_viewport(inputs(screen, 6));

    assert_eq!(decision.pin, ViewportPin::Flowing);
    assert_eq!(decision.viewport, Rect::new(0, 3, 80, 6));
}

#[test]
fn seat_bottom_pins_once_anchor_reaches_pinned_row() {
    let screen = Size::new(80, 24);
    let terminal = terminal_with_viewport(screen, Rect::new(0, 22, 80, 2));

    let decision = terminal.seat_viewport(inputs(screen, 6));

    assert_eq!(decision.pin, ViewportPin::BottomPinned);
    assert_eq!(decision.viewport, Rect::new(0, 18, 80, 6));
}

#[test]
fn seat_bottom_seated_shrink_defers_when_unbacked() {
    // A tall prompt closes with nothing to append this frame: the seat must
    // neither jump down to the pinned row (the freed rows' true content is
    // unreachable scrollback — repainting cached tail rows there duplicates
    // history still visible above) nor lift the bottom edge (coco's composer
    // is bottom-aligned; lifting it bounces the input box). It DEFERS: the
    // viewport keeps its seat, the surplus height renders as blank filler,
    // and later appends collapse it.
    let screen = Size::new(80, 24);
    let mut terminal = terminal_with_viewport(screen, Rect::new(0, 14, 80, 10));
    terminal.note_history_rows_inserted(14);

    let decision = terminal.seat_viewport(inputs(screen, 4));

    assert_eq!(decision.pin, ViewportPin::BottomPinned);
    assert_eq!(decision.viewport, Rect::new(0, 14, 80, 10));
    assert_eq!(decision.deferred_shrink_rows, 6);
}

#[test]
fn seat_bottom_seated_shrink_commits_append_backed_rows() {
    // The streaming hot path: stable rows leave the live tail by committing
    // to history in the same frame. The seat commits exactly that many rows
    // of the shrink (the append fills them before the frame presents) and
    // defers the remainder — the bottom edge never moves.
    let screen = Size::new(80, 24);
    let mut terminal = terminal_with_viewport(screen, Rect::new(0, 14, 80, 10));
    terminal.note_history_rows_inserted(14);

    let decision = terminal.seat_viewport(SeatInputs {
        guaranteed_append_rows: 4,
        ..inputs(screen, 4)
    });

    assert_eq!(decision.pin, ViewportPin::BottomPinned);
    assert_eq!(decision.viewport, Rect::new(0, 18, 80, 6));
    assert_eq!(decision.deferred_shrink_rows, 2);
}

#[test]
fn seat_bottom_seated_shrink_commits_fully_when_append_covers_it() {
    let screen = Size::new(80, 24);
    let mut terminal = terminal_with_viewport(screen, Rect::new(0, 14, 80, 10));
    terminal.note_history_rows_inserted(14);

    let decision = terminal.seat_viewport(SeatInputs {
        guaranteed_append_rows: 9,
        ..inputs(screen, 4)
    });

    assert_eq!(decision.pin, ViewportPin::BottomPinned);
    assert_eq!(decision.viewport, Rect::new(0, 20, 80, 4));
    assert_eq!(decision.deferred_shrink_rows, 0);
}

#[test]
fn seat_grow_extends_down_while_screen_has_room() {
    // A flowing viewport grows downward from its anchored top; the seat pins
    // only once the bottom would pass the screen.
    let screen = Size::new(80, 24);
    let terminal = terminal_with_viewport(screen, Rect::new(0, 3, 80, 4));

    let decision = terminal.seat_viewport(inputs(screen, 10));

    assert_eq!(decision.pin, ViewportPin::Flowing);
    assert_eq!(decision.viewport, Rect::new(0, 3, 80, 10));
}

#[test]
fn seat_reverts_to_flowing_when_history_below_pinned_row() {
    // History shrank below the bottom-pinned row: the pin is not sticky, so
    // the viewport reverts to flowing and seats flush against history instead
    // of stranding an unbacked gap above a latched bottom position.
    let screen = Size::new(80, 40);
    let terminal = terminal_with_viewport(screen, Rect::new(0, 8, 80, 4));

    let decision = terminal.seat_viewport(inputs(screen, 4));

    assert_eq!(decision.pin, ViewportPin::Flowing);
    assert_eq!(decision.viewport, Rect::new(0, 8, 80, 4));
}

// ─── seat_geometry: pin/anchor math ──────────────────────────────────────────

#[test]
fn geometry_keeps_later_height_changes_bottom_pinned() {
    let screen = Size::new(80, 24);
    // Tall history (anchor at/above the pinned row) pins the viewport, and a
    // later height grow stays pinned because the anchor still reaches the row.
    let (first_area, first_pin) = seat_geometry(/*anchor_y*/ 22, screen, /*height*/ 4);
    let (grown_area, grown_pin) = seat_geometry(/*anchor_y*/ 20, screen, /*height*/ 10);

    assert_eq!(first_pin, ViewportPin::BottomPinned);
    assert_eq!(first_area.bottom(), 24);
    assert_eq!(grown_pin, ViewportPin::BottomPinned);
    assert_eq!(grown_area.bottom(), 24);
    assert!(
        grown_area.top() < first_area.top(),
        "after pinning, larger live surfaces grow upward from the terminal bottom"
    );
}

#[test]
fn geometry_handles_zero_height_screen() {
    let (area, pin) = seat_geometry(10, Size::new(80, 0), 0);
    assert_eq!(area, Rect::new(0, 0, 80, 0));
    assert_eq!(pin, ViewportPin::Flowing);
}

// ─── seat_viewport_height: policy clamp ─────────────────────────────────────

#[test]
fn height_uses_minimum_for_idle_composer() {
    assert_eq!(
        seat_viewport_height(Size::new(80, 24), 1, MIN_H, MAX_H),
        MIN_H
    );
}

#[test]
fn height_caps_to_max() {
    assert_eq!(
        seat_viewport_height(Size::new(80, 80), 80, MIN_H, MAX_H),
        MAX_H
    );
}

#[test]
fn height_clamps_to_small_terminal() {
    assert_eq!(seat_viewport_height(Size::new(80, 3), 12, MIN_H, MAX_H), 3);
}

#[test]
fn height_zero_screen_is_zero() {
    assert_eq!(seat_viewport_height(Size::new(80, 0), 12, MIN_H, MAX_H), 0);
}
