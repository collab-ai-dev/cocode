//! The durable goal aggregate. One versioned [`GoalSnapshot`] per session is the
//! single source of truth (§13.1); protocol and UI state are never reconstructed
//! from rendered transcript messages, and it is never copied into a compaction
//! summary as an authority.

use coco_types::SessionId;
use serde::{Deserialize, Serialize};

use crate::budget::{GoalBudget, GoalCounters, GoalUsage};
use crate::completion::{CompletionContract, CompletionPolicy, CompletionRejection};
use crate::disposition::{BlockerEvidence, WaitResolution};
use crate::id::{GoalId, SpecRevision, StateVersion, Timestamp};
use crate::plan::GoalPlanRef;
use crate::status::{GoalLifecycle, GoalStatus};
use crate::text::BoundedText;

/// Snapshot schema version. Bumped only on an incompatible field change.
pub const SCHEMA_VERSION: u32 = 1;

/// Reference to a session-owned artifact materialized from a rich objective input
/// (large paste, image). The snapshot stores opaque ids, never TUI paste handles or
/// model-supplied paths (§9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub label: BoundedText,
}

/// A user-approved change to the objective, recorded with the spec revision it
/// landed at. The original objective text stays immutable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amendment {
    pub text: BoundedText,
    pub spec_revision: SpecRevision,
    pub at: Timestamp,
}

/// The immutable original objective plus durable attachment references and explicit
/// user-approved amendments (§9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalObjective {
    pub text: BoundedText,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub amendments: Vec<Amendment>,
}

impl GoalObjective {
    pub fn new(text: impl AsRef<str>) -> Self {
        Self {
            text: BoundedText::objective(text),
            attachments: Vec::new(),
            amendments: Vec::new(),
        }
    }
}

/// A bounded end-of-turn progress marker fed into the next prompt (§9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressCheckpoint {
    pub summary: BoundedText,
    pub next_step: BoundedText,
    pub at: Timestamp,
}

/// The complete versioned goal aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSnapshot {
    pub schema_version: u32,
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub spec_revision: SpecRevision,
    pub state_version: StateVersion,
    pub objective: GoalObjective,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<CompletionContract>,
    pub policy: CompletionPolicy,
    pub lifecycle: GoalLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<GoalPlanRef>,
    pub budget: GoalBudget,
    pub usage: GoalUsage,
    pub counters: GoalCounters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ProgressCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection: Option<CompletionRejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_blocker: Option<BlockerEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_resolution: Option<WaitResolution>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl GoalSnapshot {
    pub fn status(&self) -> GoalStatus {
        self.lifecycle.status()
    }

    pub fn is_active(&self) -> bool {
        self.lifecycle.is_active()
    }

    pub fn is_terminal(&self) -> bool {
        self.lifecycle.is_terminal()
    }

    /// Whether the goal has an autonomous continuation owner (running/queued lease)
    /// or a registered wake — the liveness invariant `active => exactly-one-owner`
    /// checked structurally (§7.6).
    pub fn has_continuation_owner(&self) -> bool {
        matches!(
            self.lifecycle,
            GoalLifecycle::Active { .. } | GoalLifecycle::Waiting { .. }
        )
    }
}

#[cfg(test)]
#[path = "snapshot.test.rs"]
mod tests;
