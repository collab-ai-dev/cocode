//! Runtime-owned completion evidence (§10.2, §12.2).
//!
//! The runtime mints a [`GoalEvidenceRecord`] when it *accepts* a durable result
//! (tool completion, artifact write, deterministic check, external observation).
//! The model may cite an [`EvidenceId`] but can neither create a record nor wrap
//! an old result at report time to acquire fresh provenance.

use coco_types::TurnId;
use serde::{Deserialize, Serialize};

use crate::id::{ContentDigest, EvidenceId, GoalId, GoalLeaseId, Timestamp};
use crate::text::BoundedText;

/// Kind of accepted result a record binds to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceSource {
    /// A completed tool call.
    ToolResult { tool: String },
    /// A file/artifact the runtime wrote.
    ArtifactWrite,
    /// A deterministic contract check that ran.
    DeterministicCheck { check: BoundedText },
    /// A registered external-state observation (CI, PR, service state).
    ExternalObservation,
}

/// Opaque reference into existing durable tool-output / transcript / artifact
/// storage. Large output stays where it already lives; this is the bounded index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableResultRef {
    pub locator: BoundedText,
}

impl DurableResultRef {
    pub fn new(locator: impl AsRef<str>) -> Self {
        Self {
            locator: BoundedText::short(locator),
        }
    }
}

/// Bounded durable envelope binding a result to goal/lease/turn/source identity so
/// completion evidence can be ownership-checked. Provenance is captured when the
/// result is produced, not when it is cited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalEvidenceRecord {
    pub evidence_id: EvidenceId,
    pub goal_id: GoalId,
    pub lease_id: GoalLeaseId,
    pub turn_id: TurnId,
    pub source: EvidenceSource,
    pub result_ref: DurableResultRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<ContentDigest>,
    pub observed_at: Timestamp,
}

impl GoalEvidenceRecord {
    /// Whether this record was produced under `goal_id` (ownership precondition for
    /// citing it as completion proof).
    pub fn owned_by(&self, goal_id: &GoalId) -> bool {
        &self.goal_id == goal_id
    }
}

/// A worker's citation of a runtime record inside a turn report. Carries only the
/// id and a short summary; the referenced record proves provenance (§12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub evidence_id: EvidenceId,
    pub summary: BoundedText,
}

#[cfg(test)]
#[path = "evidence.test.rs"]
mod tests;
