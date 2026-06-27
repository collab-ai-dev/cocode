//! Engine-owned viewport seating (tui-v2 §6.3).
//!
//! The seat/pin decision is computed INSIDE the engine: [`seat_viewport`]
//! anchors on the owned viewport top (the previous settled seat — history
//! emission is the single seat-mover). The shell supplies per-frame *intent*
//! ([`SeatInputs`]: screen, desired height, policy bounds) and applies the
//! returned [`SeatDecision`]; overlay policy (which surface wants alt-screen)
//! stays in the shell — alt frames cover the screen and do not seat.
//!
//! Seating follows codex's inline-viewport semantics
//! (`codex-rs/tui/src/tui.rs::draw` + `insert_history.rs`):
//!
//! - **Grow** extends the viewport downward from its anchored top; only when
//!   the bottom would pass the screen does the seat pin and the top move up
//!   (the apply path scrolls history up by exactly that overflow).
//! - **Shrink** keeps the top anchored — the seat never jumps the viewport
//!   down over rows nothing can repaint: history that scrolled into native
//!   scrollback during a grow is unreachable from below, and repainting
//!   cached tail rows below history that is still visible duplicates it on
//!   screen (the permission-prompt duplication class).
//! - Subsequent history appends walk the viewport back down
//!   (`move_viewport_down_for_history`), exactly like codex's reverse-index
//!   scroll in `insert_history_lines`.
//!
//! One deliberate divergence from codex: a shrink while the viewport is
//! seated at the screen bottom commits only the rows this frame's history
//! emission is guaranteed to append (`SeatInputs::guaranteed_append_rows`)
//! and DEFERS the rest — the bottom edge stays glued to the screen bottom
//! and the surplus height renders as blank filler inside the viewport until
//! later appends back it. Codex can float the whole shrink because its
//! composer sits at the TOP of the pane; coco's composer is bottom-aligned,
//! so an unbacked shrink that lifted the bottom edge would visibly bounce
//! the input box (observed as a 1-row jiggle during streaming). Deferral
//! never repaints history — the duplication class stays structurally
//! impossible.
//!
//! Invariant owned here: **I-V1** — the viewport always seats flush on
//! finalized history (`viewport_top == history_bottom_y`); see
//! [`SurfaceTerminal::seats_flush`]. Anchored and deferred shrinks never
//! open a gap: a gap above the viewport is a stale-anchor / second-writer
//! regression regardless of pin.

use ratatui::layout::Rect;
use ratatui::layout::Size;

use crate::engine::terminal::SurfaceBackend;
use crate::engine::terminal::SurfaceTerminal;

/// Whether the inline viewport is glued to the bottom of the screen
/// (history has reached or overflowed it) or flows directly under the
/// finalized history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportPin {
    Flowing,
    BottomPinned,
}

/// Per-frame seating intent supplied by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatInputs {
    /// Current screen size.
    pub screen: Size,
    /// Content-derived viewport height the shell wants this frame.
    pub desired_height: u16,
    /// Shell policy bounds for the viewport height clamp.
    pub min_height: u16,
    pub max_height: u16,
    /// Rows this frame's history emission is guaranteed to append. A shrink
    /// while seated at the screen bottom commits only this many rows; the
    /// rest defers so the bottom-aligned composer never lifts off the screen
    /// bottom.
    pub guaranteed_append_rows: u16,
}

/// The committed seat for one frame. `viewport` is what the shell applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatDecision {
    pub pin: ViewportPin,
    pub previous_viewport: Rect,
    pub viewport: Rect,
    /// Rows of a bottom-seated shrink deferred to later appends (diagnostic;
    /// non-zero only while the viewport carries blank filler height).
    pub deferred_shrink_rows: u16,
}

impl<B> SurfaceTerminal<B>
where
    B: SurfaceBackend,
{
    /// Decide this frame's inline viewport seat. Pure — no terminal writes.
    ///
    /// Anchors on the OWNED viewport top, not `history_bottom_y()`:
    /// `history_bottom_y` mutates mid-frame (clear/insert) and the sync pass
    /// runs BEFORE the history emission, so the owned top is the previous
    /// frame's settled seat and the emission stays the single seat-mover.
    pub fn seat_viewport(&self, inputs: SeatInputs) -> SeatDecision {
        let previous_viewport = self.viewport_area();
        let height = seat_viewport_height(
            inputs.screen,
            inputs.desired_height,
            inputs.min_height,
            inputs.max_height,
        );
        let (desired, pin) = seat_geometry(previous_viewport.top(), inputs.screen, height);
        let (viewport, pin, deferred_shrink_rows) = arbitrate_bottom_seated_shrink(
            previous_viewport,
            desired,
            pin,
            inputs.screen,
            inputs.guaranteed_append_rows,
        );
        SeatDecision {
            pin,
            previous_viewport,
            viewport,
            deferred_shrink_rows,
        }
    }

    /// The I-V1 invariant evaluated against current terminal state: the
    /// viewport must sit flush on finalized history
    /// (`viewport_top == history_bottom_y`). Anchored shrinks never open a
    /// gap, so there is no pin exemption.
    pub fn seats_flush(&self) -> bool {
        self.viewport_area().top() == self.history_bottom_y()
    }
}

/// Clamp the shell's desired viewport height to its policy bounds and the
/// screen.
pub fn seat_viewport_height(
    screen: Size,
    desired_height: u16,
    min_height: u16,
    max_height: u16,
) -> u16 {
    if screen.height == 0 {
        return 0;
    }
    desired_height
        .clamp(min_height, max_height.max(min_height))
        .min(screen.height)
}

/// Convenience for shells and test harnesses: the seat rect for a bare
/// anchor. Production seating goes through [`SurfaceTerminal::seat_viewport`].
pub fn seat_viewport_area(
    anchor_y: u16,
    screen: Size,
    desired_height: u16,
    min_height: u16,
    max_height: u16,
) -> Rect {
    let height = seat_viewport_height(screen, desired_height, min_height, max_height);
    seat_geometry(anchor_y, screen, height).0
}

/// Arbitrate a shrink while the viewport is seated at the screen bottom.
///
/// The bottom edge is where coco's composer lives, so it must not lift off
/// the screen bottom over rows nothing will repaint: commit the shrink only
/// to the extent this frame's history emission backs it (the append fills
/// the freed rows in the same synchronized frame), and defer the rest by
/// keeping the top anchored — the surplus height renders as blank filler
/// inside the viewport and later appends collapse it. Never repaints
/// history, so it cannot duplicate it.
fn arbitrate_bottom_seated_shrink(
    previous: Rect,
    desired: Rect,
    pin: ViewportPin,
    screen: Size,
    guaranteed_append_rows: u16,
) -> (Rect, ViewportPin, u16) {
    // A shrink with the top anchored lifts the desired bottom off the screen
    // bottom the viewport was seated on.
    let bottom_seated_shrink =
        previous.bottom() == screen.height && desired.bottom() < screen.height;
    if !bottom_seated_shrink {
        return (desired, pin, 0);
    }
    // Rows the top must eventually move down for the desired height to seat
    // back on the screen bottom.
    let shrink_requested = screen
        .height
        .saturating_sub(desired.height)
        .saturating_sub(previous.top());
    let shrink_committed = shrink_requested.min(guaranteed_append_rows);
    let top = previous.top().saturating_add(shrink_committed);
    let viewport = Rect::new(0, top, screen.width, screen.height.saturating_sub(top));
    (
        viewport,
        ViewportPin::BottomPinned,
        shrink_requested - shrink_committed,
    )
}

/// Compute the viewport rect and pin for a frame.
///
/// The top stays anchored unless the requested height pushes the bottom past
/// the screen — only then does the seat pin and the top move up (the apply
/// path scrolls history up by that overflow, mirroring codex). A height
/// shrink therefore keeps the top anchored and frees rows BELOW the
/// viewport; it never re-pins over freed rows whose true content is
/// unreachable scrollback.
fn seat_geometry(anchor_y: u16, screen: Size, height: u16) -> (Rect, ViewportPin) {
    if screen.height == 0 {
        return (Rect::new(0, 0, screen.width, 0), ViewportPin::Flowing);
    }
    let bottom_pinned_y = screen.height.saturating_sub(height);
    let pin = if anchor_y >= bottom_pinned_y {
        ViewportPin::BottomPinned
    } else {
        ViewportPin::Flowing
    };
    let y = match pin {
        ViewportPin::Flowing => anchor_y,
        ViewportPin::BottomPinned => bottom_pinned_y,
    };
    (Rect::new(0, y, screen.width, height), pin)
}

#[cfg(test)]
#[path = "seat.test.rs"]
mod tests;
