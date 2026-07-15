//! Goal-to-plan binding. The durable goal stores only a bounded *reference* to the
//! session-owned plan artifact, never the Markdown body (§5.5). The host
//! `PlanArtifactService` resolves the id to a path and computes digests; the goal
//! store cannot persist an arbitrary filesystem path.

use serde::{Deserialize, Serialize};

use crate::id::{ContentDigest, PlanArtifactId, PlanRevision, Timestamp};
use crate::text::BoundedText;

/// Structural binding from a goal to its plan artifact (§5.5 `GoalPlanRef`).
///
/// The binding is by artifact id, not content similarity: the session owns exactly
/// one current plan artifact per slug. Digest drift is detected, never used to
/// re-decide relevance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPlanRef {
    pub artifact_id: PlanArtifactId,
    pub revision: PlanRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<ContentDigest>,
    pub observed_at: Timestamp,
}

impl GoalPlanRef {
    /// Whether `observed` reflects a different plan artifact revision/digest than
    /// this binding — i.e. the file changed outside the current worker turn.
    pub fn drifted_from(&self, observed: &GoalPlanRef) -> bool {
        self.artifact_id == observed.artifact_id
            && (self.revision != observed.revision
                || self.content_digest != observed.content_digest)
    }
}

/// End-of-turn record of the observed plan revision/digest plus a short progress
/// note. Supports drift detection and the next prompt; the file stays the content
/// authority (§5.5 `PlanCheckpoint`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCheckpoint {
    pub revision: PlanRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<ContentDigest>,
    pub summary: BoundedText,
    pub observed_at: Timestamp,
}

#[cfg(test)]
#[path = "plan.test.rs"]
mod tests;
