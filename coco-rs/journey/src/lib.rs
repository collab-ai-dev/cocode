//! coco-journey — the read-side assembler for the `/journey` learning timeline.
//!
//! Mirrors the responsibility boundary of Hermes' `learning_graph`: a pure
//! disk-scan (+ journal-merge) that assembles a [`JourneySnapshot`] of *learned
//! skills + memories* over time. No I/O policy, no TUI, no domain mutation —
//! just read, merge, and bucketize. Consumed only by `app/cli` (the one place
//! that already carries both `coco-skills` and `coco-memory`).
//!
//! - [`snapshot`] — `build_journey`: agent-skill scan + user/project skill
//!   selection + memory scan + telemetry join (+ journal merge, wired in the
//!   journal-substrate work package).
//! - [`timeline`] — pure `bucketize`: day → month → year adaptive granularity
//!   with a recency-driven ink signal. Clock is injected (no `SystemTime`).

pub mod snapshot;
pub mod timeline;

pub use snapshot::{
    AgentSkillLifecycle, JourneyNode, JourneyNodeBody, JourneyPaths, JourneySnapshot, JourneyStats,
    build_journey,
};
pub use timeline::{TimelineBucket, bucketize, day_label};
