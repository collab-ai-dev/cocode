//! Canonical "Arc → owned → mutate → Arc" pipeline for message
//! transformations.
//!
//! ## Why this module exists
//!
//! Two crates (`coco-messages` and `coco-compact`) need to chain
//! in-place mutating passes (filter / merge / strip etc.) over
//! `Vec<Arc<Message>>` history snapshots. Without a canonical wrapper
//! each call site reinvents the same `let owned = ...; mutate; re-wrap`
//! boilerplate, plus an ad-hoc "would any pass mutate?" predicate to
//! skip the materialize when no work is needed. The result was three
//! scattered entry materializes (`compact_conversation`,
//! `partial_compact_conversation`, `compact_session_memory`) and one
//! centralized predicate (`needs_message_level_passes`) that duplicated
//! the trigger logic of seven helper functions.
//!
//! This module collapses all of that into:
//!
//! 1. [`MessagePass`] — one trait per mutating algorithm. Each
//!    implementer bundles its own `would_mutate` predicate alongside
//!    `apply`, so the trigger condition lives next to the mutation. No
//!    central predicate ever drifts out of sync.
//! 2. [`run_message_passes`] — the single canonical bridge. Caller
//!    chooses which passes to run by composing them in an explicit
//!    pipeline function (static dispatch — each pass is a concrete
//!    type, no `dyn` indirection).
//!
//! ## Fast path / slow path
//!
//! `run_message_passes` walks the input slice once to ask each pass
//! whether it would do work. If **none** would mutate, it returns
//! `input.to_vec()` — N atomic refcount bumps, zero `Message::clone`.
//! Otherwise it materializes a `Vec<Message>` once, runs all
//! `apply`s in caller-specified order, and re-wraps into a fresh
//! Arc-vec.
//!
//! ## Pipeline shape
//!
//! Each in-place pass maps 1:1 to a Rust [`MessagePass`] impl; the
//! explicit pipeline mirrors the `array.filter().map()` chain shape of
//! `normalizeMessagesForAPI` and the `stripImagesFromMessages` →
//! `stripReinjectedAttachments` chain in compact.

use std::sync::Arc;

use crate::Message;

/// A mutating message-level pass.
///
/// Implementers expose a cheap [`would_mutate`](MessagePass::would_mutate)
/// predicate so [`run_message_passes`] can skip the materialize when
/// no pass would do work. The actual mutation logic lives in
/// [`apply`](MessagePass::apply), which is only invoked on the slow
/// path after one materialize.
///
/// Keep `would_mutate` strictly cheaper than `apply` — single walk, no
/// allocation, no clone. The contract is "**if would_mutate returns
/// false, `apply` would have been a no-op**". False positives (the
/// predicate over-reports) are allowed and slow the pipeline down by
/// one materialize, but never produce wrong output. False negatives
/// silently skip the pass and ARE bugs.
pub trait MessagePass {
    /// Cheap predicate: would [`apply`](Self::apply) mutate this slice?
    ///
    /// Receives borrowed `&Message` refs to avoid materializing
    /// `Vec<Message>` for the check. Must be referentially transparent
    /// — same input ⇒ same output.
    fn would_mutate(&self, messages: &[&Message]) -> bool;

    /// In-place mutation. Only invoked when at least one pass in the
    /// pipeline reported `would_mutate == true`.
    fn apply(&self, messages: &mut Vec<Message>);
}

/// Canonical "Arc-vec → owned → mutate → Arc-vec" bridge.
///
/// `needs_mutate` is the **combined** decision over the pipeline's
/// passes — computed by the caller (typically as `P1.would_mutate() ||
/// P2.would_mutate() || …`) so the predicate walks happen on borrowed
/// refs and short-circuit cheaply.
///
/// When `false`: returns `input.to_vec()` — N×`Arc::clone`, zero
/// `Message::clone`. Equivalent to a shallow array copy of unchanged
/// object refs.
///
/// When `true`: materializes one owned `Vec<Message>`, hands it to
/// `apply_all` (which calls each pass's `apply` in order), then
/// re-wraps into a fresh `Vec<Arc<Message>>`.
pub fn run_message_passes<F>(
    input: &[Arc<Message>],
    needs_mutate: bool,
    apply_all: F,
) -> Vec<Arc<Message>>
where
    F: FnOnce(&mut Vec<Message>),
{
    if !needs_mutate {
        return input.to_vec();
    }
    let mut owned: Vec<Message> = input.iter().map(|a| (**a).clone()).collect();
    apply_all(&mut owned);
    owned.into_iter().map(Arc::new).collect()
}

/// Build a borrowed-ref view of an Arc-vec for the [`MessagePass::would_mutate`]
/// scan. One walk; the resulting `Vec<&Message>` is consumed by the
/// pipeline's combined predicate expression.
pub fn borrow_refs(input: &[Arc<Message>]) -> Vec<&Message> {
    input.iter().map(Arc::as_ref).collect()
}
