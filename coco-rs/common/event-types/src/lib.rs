//! The three-layer `CoreEvent` envelope and every wire payload it carries.
//!
//! Split out of `coco-types` because the event surface is the workspace's
//! highest-churn API (≈1 commit in 6 touches it) while `coco-types` is its
//! widest dependency (44 crates). Keeping them in one crate meant every
//! dialog payload tweak recompiled `coco-config`, `coco-tools`,
//! `coco-inference` and everything downstream of them. Only the ~22 crates
//! that actually speak the wire protocol depend on this one.
//!
//! Design: `docs/internal/event-system-design.md`.

mod event;
pub use event::*;

mod session_access;
mod stream_accumulator;
pub use session_access::{
    ServerRequestDelivery, SessionAccess, SessionDelivery, SessionLifecycleEffect,
    SessionLifecycleEffectKind,
};
pub use stream_accumulator::StreamAccumulator;
