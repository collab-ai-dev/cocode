//! Paste-burst detection for terminals without bracketed paste.
//!
//! coco enables bracketed paste at startup, and where the terminal honours it a
//! paste arrives as one `Event::Paste`. Where it does not — Windows conhost,
//! some SSH and multiplexer configurations, older emulators — the same paste
//! arrives as a rapid stream of individual `Char` and `Enter` key events. Every
//! `Enter` in that stream then reads as "submit", so pasting a five-line prompt
//! sends five messages.
//!
//! This tracker recognizes that stream by timing alone: characters arriving
//! faster than a human types, several in a row, open a *burst window*. While the
//! window is open, `Enter` means "newline inside the pasted text" rather than
//! "submit". The window extends with each burst-like key and closes once input
//! goes quiet.
//!
//! # Scope
//!
//! The tracker deliberately buffers nothing and rewrites no text. Characters are
//! inserted as they arrive, exactly as before; only the meaning of `Enter`
//! changes. That keeps a misfire harmless — the worst case is a newline the user
//! has to delete, never lost or reordered input — and keeps the editor out of
//! the state machine. Coalescing a paste into one attachment (the large-paste
//! pill) remains a bracketed-paste-only feature, because only bracketed paste
//! delimits where the paste ends.
//!
//! # Tuning
//!
//! [`BURST_CHAR_INTERVAL`] is the ceiling on the gap between two characters for
//! them to count as one burst, and [`BURST_MIN_CHARS`] is how many such
//! characters must arrive before the window opens. The defaults are set far
//! outside human reach rather than merely above it: a paste's characters arrive
//! in one terminal read, microseconds apart, so the detector does not need a
//! tight threshold to catch one — but a fast typist who finishes a word and
//! immediately hits Enter must never trip it.

use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;

/// Maximum gap between two characters for them to belong to the same burst.
///
/// 50 characters per second. Sustained human typing peaks near 10, and a burst
/// digraph near 20; pasted characters arrive in a single read, far under 1ms.
pub const BURST_CHAR_INTERVAL: Duration = Duration::from_millis(20);

/// Consecutive fast characters required before the burst window opens.
///
/// Four characters means three consecutive sub-20ms gaps — unreachable by hand,
/// and met by the first word of any paste.
pub const BURST_MIN_CHARS: usize = 4;

/// How long the window stays open after the last burst-like key.
///
/// This is what a paste's own internal pauses (terminal write chunking, a slow
/// SSH link) have to fit inside, so it is deliberately longer than
/// [`BURST_CHAR_INTERVAL`].
pub const BURST_IDLE_TIMEOUT: Duration = Duration::from_millis(200);

/// Tracks whether recent key input looks like a paste rather than typing.
#[derive(Debug, Default, Clone)]
pub struct PasteBurst {
    /// Timestamp of the most recent plain character.
    last_char_at: Option<Instant>,
    /// How many plain characters have arrived back-to-back inside
    /// [`BURST_CHAR_INTERVAL`], saturating at [`BURST_MIN_CHARS`].
    consecutive_fast_chars: usize,
    /// When the open burst window expires. `None` while no window is open.
    window_until: Option<Instant>,
}

impl PasteBurst {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a key press and update the burst window.
    ///
    /// Feed every key event the app receives, before dispatch. Keys that cannot
    /// appear inside a paste (anything with Ctrl/Alt, function keys, arrows)
    /// close the window: a human reached for them, so the paste is over.
    pub fn observe(&mut self, key: KeyEvent, now: Instant) {
        match key.kind {
            KeyEventKind::Press => {}
            // Auto-repeat proves a key is being held down, which is the one
            // other way to produce a fast character stream. It is not a paste.
            KeyEventKind::Repeat => {
                self.close();
                return;
            }
            KeyEventKind::Release => return,
        }
        match key.code {
            KeyCode::Char(_) if is_plain(key) => self.observe_plain_char(now),
            // Newlines and tabs are ordinary paste content and keep an open
            // window alive, but they are never evidence *for* a burst: they may
            // only extend a window the characters already opened.
            KeyCode::Enter | KeyCode::Tab if is_plain(key) => {
                if self.is_bursting(now) {
                    self.window_until = Some(now + BURST_IDLE_TIMEOUT);
                }
            }
            _ => self.close(),
        }
    }

    /// Whether input is currently mid-paste, as of `now`.
    pub fn is_bursting(&self, now: Instant) -> bool {
        self.window_until.is_some_and(|until| now < until)
    }

    /// Forget any burst state. Internal: the window is time-bounded, so
    /// callers never need to close it by hand — only a key that proves a human
    /// took over does.
    fn close(&mut self) {
        *self = Self::default();
    }

    fn observe_plain_char(&mut self, now: Instant) {
        let is_fast = self
            .last_char_at
            .is_some_and(|last| now.saturating_duration_since(last) <= BURST_CHAR_INTERVAL);
        self.last_char_at = Some(now);
        self.consecutive_fast_chars = if is_fast {
            self.consecutive_fast_chars.saturating_add(1)
        } else {
            1
        };
        if self.consecutive_fast_chars >= BURST_MIN_CHARS {
            self.window_until = Some(now + BURST_IDLE_TIMEOUT);
        }
    }
}

/// Whether the key carries no modifier a paste could not produce.
///
/// SHIFT is allowed because pasted uppercase and symbols arrive shifted on many
/// terminals; CONTROL / ALT / SUPER are deliberate human chords.
fn is_plain(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

#[cfg(test)]
#[path = "paste_burst.test.rs"]
mod tests;
