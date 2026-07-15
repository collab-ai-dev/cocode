//! Branded identity and revision newtypes for the goal aggregate.
//!
//! The pure reducer never mints these; the host generates opaque string ids and
//! advances the monotonic counters, then threads them in through commands. This
//! keeps the domain layer deterministic and free of `uuid`/clock I/O.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Declare an opaque, transparent string newtype used as a durable identifier.
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }
    };
}

string_id! {
    /// Durable goal identity. A replacement goal always receives a fresh id even
    /// when the objective text is identical (§9.1 invariant 7).
    GoalId
}

string_id! {
    /// Identifies one goal-owned work attempt (queued or running) inside the
    /// owning runtime. Distinct from the cross-process session write lease.
    GoalLeaseId
}

string_id! {
    /// Identity of a registered wake obligation (task/deadline/mode/…).
    WakeId
}

string_id! {
    /// Idempotency key for a single lifecycle effect (accounting delta, start,
    /// completion) so at-least-once delivery cannot double-apply.
    EffectId
}

string_id! {
    /// Runtime-issued provenance identity for an accepted evidence result. The
    /// model may cite one but can never mint or rebind it.
    EvidenceId
}

string_id! {
    /// Session-owned opaque identifier for the current plan artifact. Resolved to
    /// a filesystem path only by the host `PlanArtifactService`.
    PlanArtifactId
}

string_id! {
    /// Durable identity of one completion-verification attempt, so a crash after
    /// the call but before persistence can safely retry.
    VerificationAttemptId
}

/// Content hash of a bounded byte snapshot. Detects change and stale excerpts; it
/// is not a security boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentDigest(String);

impl ContentDigest {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Monotonic revision of the user-authored goal specification (objective,
/// contract, budget, plan binding). Only a user edit advances it. Optimistic
/// concurrency: an edit compares its `expected_spec_revision` against this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecRevision(u32);

impl SpecRevision {
    /// First revision minted at goal creation.
    pub const INITIAL: Self = Self(1);

    pub fn get(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for SpecRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Monotonic per-goal write ordering key. Advances on every committed runtime
/// transition and is the event-order key. It orders writes *inside* one
/// exclusive session writer and is not a substitute for the session lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateVersion(u64);

impl StateVersion {
    /// Version stamped by the creating transition.
    pub const INITIAL: Self = Self(0);

    pub fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for StateVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Monotonic observed revision of the plan artifact. Advances only when a new
/// digest is accepted. Independent of `SpecRevision`: a plan edit never changes
/// the objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanRevision(u64);

impl PlanRevision {
    pub const INITIAL: Self = Self(0);

    pub fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for PlanRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Wall-clock instant as Unix milliseconds. The host reads the clock and passes
/// it in; the reducer only stores and compares. Monotonic durations are
/// accounted separately in [`crate::budget`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub fn millis(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
#[path = "id.test.rs"]
mod tests;
