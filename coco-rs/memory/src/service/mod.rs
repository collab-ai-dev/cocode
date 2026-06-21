//! Async services driven by the agent loop.
//!
//! - [`extract`] — turn-end memory extraction (forked subagent)
//! - [`dream`] — periodic auto-dream consolidation (forked subagent)
//! - [`session`] — per-session 9-section markdown insights

pub mod dream;
pub mod extract;
pub mod session;

pub use dream::DreamService;
pub use extract::ExtractService;
pub use session::SessionMemoryService;

#[cfg(test)]
pub(super) mod test_support;
