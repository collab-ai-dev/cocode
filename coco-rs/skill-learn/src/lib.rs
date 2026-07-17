//! Skill autonomous learning loop — the capability-layer analogue of the
//! memory loop.
//!
//! Mirrors `coco-memory`'s structure (fenced background review + periodic
//! consolidation) but pointed at *skills* instead of *knowledge*:
//!
//! - [`fence`] — the [`fence::SkillWriteHandle`] `CanUseToolHandle` policy that
//!   confines a review fork to writing under the dedicated agent skills
//!   directory (spatial containment; the shared L0 primitives do the actual
//!   symlink-aware / read-only-bash checks). The directory itself is owned by
//!   `coco_skills::agent_scope`, which also does the location-keyed
//!   inert-load + quarantine enforcement on the read side.
//! - [`review`] — the turn-end review fork + trusted provenance stamping.
//! - [`runtime`] — throttle + single-flight trigger driven by the engine.
//! - [`curator`] — periodic retire/promote pass over agent skills.
//!
//! Provenance and telemetry live in `coco-skills` (the data plane). LLM +
//! spawn interaction is only ever through the `coco-tool-runtime` traits, so
//! this crate does not depend on `coco-messages` / `coco-inference` (same
//! layering as memory). Public entry points report through outcome enums
//! ([`CuratorOutcome`], [`SkillReviewOutcome`], [`ReviewTrigger`]) — the loop
//! is fire-and-forget from the engine's perspective, so there is no
//! crate-level error type.

pub mod curator;
pub mod fence;
pub mod journal;
pub mod notice;
pub mod review;
pub mod runtime;
mod stamp;

pub use curator::{CuratorOutcome, SkillCurator};
pub use notice::{SkillLearnInbox, SkillLearnNotice, SkillLearnVerb};
pub use review::{AgentSlot, SkillReviewOutcome, SkillReviewService};
pub use runtime::{ReviewSignal, ReviewTrigger, SkillReviewRuntime};
