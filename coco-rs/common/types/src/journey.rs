//! Learning-timeline event schema — the shared vocabulary of the append-only
//! journal that backs the `/journey` view.
//!
//! Four trusted host-side write sites (skill `stamp`, skill `curator`, memory
//! `extract`, memory `dream`) append [`JourneyRecord`]s to a JSONL journal;
//! the `coco-journey` read side merges them into a timeline. The types are
//! shared by skill-learn, memory, journey, and app/tui — hence they live here
//! (house rule: a type consumed by 3+ crates belongs in `coco-types`).
//!
//! Two families:
//! - **Wire events** ([`JourneyRecord`] / [`JourneyEvent`] / [`SkillRetireReason`])
//!   are `Serialize`/`Deserialize` — they cross the process boundary onto disk.
//! - **In-process addressing** ([`JourneyNodeId`] / [`JourneyAction`]) carry
//!   `PathBuf` and are moved over the in-process `UserCommand` mpsc, never
//!   serialized (D9): the wire payload mirrors them with `String` paths.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One fact on the learning timeline. Append-only; never rewritten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JourneyRecord {
    /// Event time (epoch ms). Write sites use their own clock; forks are never
    /// trusted to stamp this.
    pub at_ms: i64,
    /// Originating session (audit / backtrace); `None` when absent (e.g. a
    /// curator tick that carries no session context).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub event: JourneyEvent,
}

impl JourneyRecord {
    /// Build a record stamping `at_ms` and an optional `session_id`.
    pub fn new(at_ms: i64, session_id: Option<String>, event: JourneyEvent) -> Self {
        Self {
            at_ms,
            session_id,
            event,
        }
    }
}

/// A single learning-timeline event. Wire-tagged on `event` (closed set —
/// no bare strings for discriminators).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JourneyEvent {
    /// Review fork created a new agent skill (stamp saw no prior `created-at`).
    SkillLearned { name: String },
    /// Review fork updated an existing agent skill or its support files.
    SkillUpdated { name: String },
    /// Curator promoted a skill on telemetry (≥5 invocations, success ≥ 0.8).
    SkillPromoted { name: String },
    /// Curator or user retired the skill (`disabled: true` flip).
    SkillRetired {
        name: String,
        reason: SkillRetireReason,
    },
    /// User restored a retired skill via `/journey` (`disabled` flipped back).
    SkillRestored { name: String },
    /// Memory extract fork wrote topic files (memdir-relative paths).
    MemoryWritten { files: Vec<String> },
    /// Dream consolidation touched files (aggregate fact for merge/rename/delete).
    MemoryConsolidated { files_touched: i32 },
    /// User deleted a memory via `/journey`.
    MemoryDeleted { file: String },
}

impl JourneyEvent {
    /// The skill name this event concerns, when it is a skill event.
    pub fn skill_name(&self) -> Option<&str> {
        match self {
            Self::SkillLearned { name }
            | Self::SkillUpdated { name }
            | Self::SkillPromoted { name }
            | Self::SkillRetired { name, .. }
            | Self::SkillRestored { name } => Some(name),
            Self::MemoryWritten { .. }
            | Self::MemoryConsolidated { .. }
            | Self::MemoryDeleted { .. } => None,
        }
    }
}

/// Why a skill was retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRetireReason {
    /// ≥5 invocations with success rate below the retire threshold.
    FailureRate,
    /// Previously used, then idle past the inactivity horizon.
    Inactivity,
    /// User action via `/journey`.
    Manual,
}

impl SkillRetireReason {
    /// Stable discriminator string (for i18n keys / logging).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FailureRate => "failure_rate",
            Self::Inactivity => "inactivity",
            Self::Manual => "manual",
        }
    }
}

/// Typed node addressing (fixes the "no stable node identity" gap): canonical
/// `SKILL.md` path for skills (unambiguous across same-named scopes),
/// memdir-relative filename for memories. In-process type — the wire payload
/// mirrors it with `String` paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JourneyNodeId {
    Skill { path: PathBuf },
    Memory { filename: String },
}

/// In-process mutation request carried by `UserCommand` (no serde; D9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JourneyAction {
    /// Flip a skill to `disabled: true`.
    RetireSkill { path: PathBuf },
    /// Flip a skill back to enabled.
    RestoreSkill { path: PathBuf },
    /// Hard-delete a memory topic file (memdir-relative filename).
    DeleteMemory { filename: String },
    /// Open a node's backing file in the external editor.
    OpenInEditor { id: JourneyNodeId },
}

#[cfg(test)]
#[path = "journey.test.rs"]
mod tests;
