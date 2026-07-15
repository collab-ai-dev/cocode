//! Wire-facing goal control-plane DTOs (design §8). Bounded projections of the
//! durable `coco_goals::GoalSnapshot`, so `coco-types` stays free of a
//! `coco-goals` dependency; `coco-agent-host` maps the domain aggregate into
//! these views at the protocol boundary.

use serde::{Deserialize, Serialize};

use crate::SessionTarget;

/// Wire status of a goal, mirroring `coco_goals::GoalStatus`. A cleared goal is
/// represented by a `None` snapshot on [`GoalSnapshotChangedParams`], so there is
/// no `Cleared` variant here.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatusKind {
    Active,
    Waiting,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Completed,
}

impl GoalStatusKind {
    /// A stopped status needing an explicit resume/edit action — not terminal and
    /// not automatically continuing.
    pub fn is_stopped(self) -> bool {
        matches!(
            self,
            Self::Paused | Self::Blocked | Self::UsageLimited | Self::BudgetLimited
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Bounded wire projection of a durable goal snapshot for protocol and TUI
/// consumers. Never reconstructed from transcript messages (design §9.1).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSnapshotView {
    pub goal_id: String,
    pub spec_revision: u64,
    pub state_version: u64,
    pub status: GoalStatusKind,
    /// Typed reason for a stopped/waiting status (pause reason, blocker summary,
    /// wait condition, budget kind). `None` for a plain running goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_detail: Option<String>,
    pub objective: String,
    pub total_turns: i32,
    pub autonomous_turns: i32,
    pub max_autonomous_turns: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Full-snapshot goal event payload (design §8.1). Session routing rides the
/// `SessionEnvelope` the emit site stamps, so there is no payload session id.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSnapshotChangedParams {
    /// The current goal projection, or `None` when no goal exists (cleared).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<GoalSnapshotView>,
}

/// `session/goal/create` parameters. Minimal control-plane surface; contract and
/// rich budget authoring beyond the autonomous-turn cap are future extensions.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCreateParams {
    pub target: SessionTarget,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_autonomous_turns: Option<i32>,
}

/// `session/goal/edit` parameters. `expected_spec_revision` is the optimistic
/// concurrency guard (design §9.1 invariant 2): the edit is rejected when it does
/// not match the current spec revision.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalEditParams {
    pub target: SessionTarget,
    pub expected_spec_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_autonomous_turns: Option<i32>,
}

/// The controlled status transition a `session/goal/setStatus` requests. Only
/// user-drivable transitions are exposed; terminal and verifier-owned statuses
/// are not (design §6, §11.3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatusRequest {
    Pause,
    Resume,
}

/// `session/goal/setStatus` parameters.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSetStatusParams {
    pub target: SessionTarget,
    pub status: GoalStatusRequest,
}

/// Result of a goal control-plane RPC: the current snapshot projection, or `None`
/// after a clear.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCommandResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<GoalSnapshotView>,
}
