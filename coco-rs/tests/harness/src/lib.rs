//! Shared test harness for coco-rs integration tests.
//!
//! Provides message builders, conversation constructors, mock summarize_fn
//! factories for compact e2e testing, and volatile-field normalization for
//! golden snapshots.

#[allow(clippy::type_complexity)]
pub mod compact;
pub mod conversation;
pub mod messages;
pub mod normalize;
pub mod recording;
pub mod registry;
