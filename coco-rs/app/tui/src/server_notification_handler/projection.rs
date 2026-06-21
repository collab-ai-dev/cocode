//! Shared projections from transient UI state into transcript messages.
//!
//! The engine's `Message::Assistant` pushes flow through
//! `MessageAppended` → `TranscriptView`; the viewport merge in
//! `surface/viewport.rs` renders them from transcript directly. This
//! helper only clears the `ui.streaming` overlay buffer so the
//! streaming-tail widget stops drawing the live deltas once the engine
//! has committed them.

use crate::state::AppState;

pub(super) fn flush_streaming_to_messages(state: &mut AppState) {
    // Drop any in-flight streaming deltas; engine-pushed
    // Message::Assistant is the authoritative record (rendered via
    // transcript).
    state.ui.streaming = None;
}
