//! Keybinding resolver. Given a stack of active contexts and an incoming key
//! combo, resolve to an action — possibly after holding state across multiple
//! combos for chord progressions.
//!
//! Outcomes:
//!
//! * [`ResolveOutcome::NoMatch`] — no binding fired; pending state was
//!   reset.
//! * [`ResolveOutcome::Fire`] — a complete chord matched; fire the
//!   action.
//! * [`ResolveOutcome::Pending`] — a chord prefix matched; hold for
//!   the next combo. The caller must consult [`ChordResolver::tick`]
//!   periodically so the pending state times out (1-second window).
//! * [`ResolveOutcome::Unbound`] — the user explicitly null-bound this
//!   chord; the caller should swallow the keystroke.
//! * [`ResolveOutcome::ChordCancelled`] — a pending chord was cancelled
//!   (Esc, timeout, or non-matching follow-up). Used by the TUI to clear
//!   the chord status indicator.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use crate::Keybinding;
use crate::KeybindingAction;
use crate::KeybindingContext;
use crate::parser::KeyChord;
use crate::parser::KeyCombo;

/// How long a partial chord is allowed to wait for its next combo.
pub const CHORD_TIMEOUT: Duration = Duration::from_millis(1000);

/// Result of feeding one combo into the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// No matching binding; resolver state was reset.
    NoMatch,
    /// A complete chord matched; fire this action.
    Fire(KeybindingAction),
    /// A chord prefix matched; hold for the next combo.
    Pending,
    /// A complete chord matched a `null`-bound entry. The caller should
    /// swallow the keystroke and not fall through.
    Unbound,
    /// A pending chord was cancelled (Esc / timeout / non-matching
    /// follow-up). The caller should clear any chord status indicator.
    ChordCancelled,
}

/// Compiled keybinding map, indexed by context for cheap lookup.
///
/// Each binding is stored as `(KeyChord, Option<KeybindingAction>)`; the
/// `None` action represents a null unbind.
#[derive(Debug)]
pub struct ChordResolver {
    by_context: HashMap<KeybindingContext, Vec<(KeyChord, Option<KeybindingAction>)>>,
    pending: Vec<KeyCombo>,
    chord_started_at: Option<Instant>,
}

impl ChordResolver {
    /// Build a resolver from a flat list of parsed bindings. Order is
    /// preserved within each context — within `feed`, last-matching wins.
    pub fn new(bindings: &[Keybinding]) -> Self {
        let mut by_context: HashMap<KeybindingContext, Vec<(KeyChord, Option<KeybindingAction>)>> =
            HashMap::new();
        for b in bindings {
            by_context
                .entry(b.context)
                .or_default()
                .push((b.chord.clone(), b.action.clone()));
        }
        Self {
            by_context,
            pending: Vec::new(),
            chord_started_at: None,
        }
    }

    /// Feed a combo. `context_stack` is the list of currently-active
    /// contexts in order of priority (most-specific first); `Global`
    /// must be appended by the caller if it should also match.
    #[tracing::instrument(
        level = "trace",
        skip(self, context_stack),
        fields(
            key = %combo.key,
            ctx_count = context_stack.len(),
            pending = self.pending.len(),
        ),
    )]
    pub fn feed(
        &mut self,
        combo: &KeyCombo,
        context_stack: &[KeybindingContext],
    ) -> ResolveOutcome {
        // Esc cancels a pending chord.
        if !self.pending.is_empty() && is_escape(combo) {
            tracing::trace!(pending_steps = self.pending.len(), "chord cancelled by Esc",);
            self.clear_state();
            return ResolveOutcome::ChordCancelled;
        }

        let was_pending = !self.pending.is_empty();
        self.pending.push(combo.clone());
        let attempt = self.pending.clone();

        let mut prefix_match = false;
        let mut exact_match: Option<Option<KeybindingAction>> = None;
        'search: for ctx in context_stack {
            if let Some(bindings) = self.by_context.get(ctx) {
                for (chord, action) in bindings {
                    if combo_slice_equals(&chord.0, &attempt) {
                        exact_match = Some(action.clone());
                        break 'search;
                    }
                    if combo_slice_is_prefix(&chord.0, &attempt) {
                        prefix_match = true;
                    }
                }
            }
        }

        if let Some(action) = exact_match {
            self.clear_state();
            return match action {
                Some(action) => {
                    tracing::trace!(
                        action = %action,
                        chord_steps = attempt.len(),
                        "action fired",
                    );
                    ResolveOutcome::Fire(action)
                }
                None => {
                    tracing::trace!(
                        chord_steps = attempt.len(),
                        "chord matched a null-bind (unbound)",
                    );
                    ResolveOutcome::Unbound
                }
            };
        }

        if prefix_match {
            // Refresh timeout deadline — every successful chord step
            // resets the 1-second window.
            self.chord_started_at = Some(Instant::now());
            tracing::trace!(
                pending_steps = attempt.len(),
                "chord prefix matched; awaiting next combo",
            );
            ResolveOutcome::Pending
        } else if was_pending {
            // Pending chord broken by an unmatched follow-up.
            tracing::trace!(
                aborted_steps = attempt.len() - 1,
                "chord cancelled by unmatched follow-up",
            );
            self.clear_state();
            ResolveOutcome::ChordCancelled
        } else {
            self.clear_state();
            ResolveOutcome::NoMatch
        }
    }

    /// Tick: check whether a pending chord has timed out. Returns
    /// `Some(ChordCancelled)` if the 1-second window has elapsed since
    /// the last combo, otherwise `None`.
    ///
    /// The TUI loop should call this on each tick (or just before the
    /// next render) to clear stale chord state.
    pub fn tick(&mut self, now: Instant) -> Option<ResolveOutcome> {
        match self.chord_started_at {
            Some(started) if now.saturating_duration_since(started) >= CHORD_TIMEOUT => {
                tracing::debug!(
                    elapsed_ms = now.saturating_duration_since(started).as_millis() as u64,
                    "chord timeout cancelled pending state",
                );
                self.clear_state();
                Some(ResolveOutcome::ChordCancelled)
            }
            _ => None,
        }
    }

    /// Reset any pending chord state. Call when focus changes or the
    /// caller wants to abort a chord without reporting it as cancelled.
    pub fn reset(&mut self) {
        self.clear_state();
    }

    /// Whether a partial chord is in flight.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The combos that have been fed but not yet resolved. Useful for
    /// rendering "ctrl+x …" in the status bar.
    pub fn pending_combos(&self) -> &[KeyCombo] {
        &self.pending
    }

    /// Pre-formatted "ctrl+x …" hint for the status bar.
    /// Returns `None` if no chord is pending.
    ///
    /// `platform` controls modifier rendering (`opt`/`alt`,
    /// `cmd`/`super`).
    pub fn pending_display(&self, platform: crate::display::DisplayPlatform) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let mut out = self
            .pending
            .iter()
            .map(|c| crate::display::keystroke_to_display_string(c, platform))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(" …");
        Some(out)
    }

    /// Look up the chord bound to `action` in the most-specific active
    /// context. Used for rendering shortcut hints in the status bar / help.
    ///
    /// Returns `None` if the action isn't bound in any of the listed
    /// contexts.
    pub fn display_for(
        &self,
        action: &KeybindingAction,
        context_stack: &[KeybindingContext],
        platform: crate::display::DisplayPlatform,
    ) -> Option<String> {
        for ctx in context_stack {
            if let Some(bindings) = self.by_context.get(ctx) {
                // Last-wins so user overrides beat earlier defaults.
                for (chord, bound) in bindings.iter().rev() {
                    if bound.as_ref() == Some(action) {
                        return Some(crate::display::chord_to_display_string(chord, platform));
                    }
                }
            }
        }
        None
    }

    fn clear_state(&mut self) {
        self.pending.clear();
        self.chord_started_at = None;
    }
}

fn is_escape(combo: &KeyCombo) -> bool {
    combo.key == "escape" && !combo.ctrl && !combo.alt && !combo.shift && !combo.meta
}

fn combo_slice_equals(chord: &[KeyCombo], attempt: &[KeyCombo]) -> bool {
    chord == attempt
}

fn combo_slice_is_prefix(chord: &[KeyCombo], attempt: &[KeyCombo]) -> bool {
    chord.len() > attempt.len() && chord[..attempt.len()] == attempt[..]
}

#[cfg(test)]
#[path = "resolver.test.rs"]
mod tests;
