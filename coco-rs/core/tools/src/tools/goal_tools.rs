//! Goal tools — `get_goal`, `report_goal_turn`, `create_goal` (§12.1).
//!
//! Lease-bound protocol tools that reach the session's goal runtime through
//! `ctx.goal` ([`coco_tool_runtime::GoalHandle`]). `get_goal` / `report_goal_turn`
//! are visible only while a live goal exists (`is_enabled`), so the model sees
//! them exactly when they apply. The worker submits its turn disposition through
//! `report_goal_turn`; the coordinator evaluates it at turn finalization.

use coco_goals::{
    BlockerEvidence, BoundedText, EvidenceId, EvidenceRef, GoalTurnDisposition,
    RequirementCoverage, RequirementResult, WaitCondition,
};
use coco_messages::ToolResult;
use coco_tool_runtime::{
    DescriptionOptions, GoalCreateRequest, PromptOptions, Tool, ToolError, ToolResultContentPart,
    ToolUseContext,
};
use coco_types::{ToolId, ToolName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── get_goal ───────────────────────────────────────────────────────────────

const GET_GOAL_PROMPT: &str = "Read the current goal: its objective, status, budget, usage, and completion policy. Use this to re-orient before acting on the goal.";

/// Empty input for [`GetGoalTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetGoalInput {}

/// A bounded projection of the goal snapshot for the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetGoalOutput {
    pub objective: String,
    pub status: String,
    pub autonomous_turns_used: u32,
    pub autonomous_turns_max: u32,
    pub total_turns: u32,
    pub tokens_used: u64,
    pub has_contract: bool,
}

pub struct GetGoalTool;

#[async_trait::async_trait]
impl Tool for GetGoalTool {
    type Input = GetGoalInput;
    coco_tool_runtime::impl_runtime_schema!(GetGoalInput);
    type Output = GetGoalOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::GetGoal)
    }
    fn name(&self) -> &str {
        ToolName::GetGoal.as_str()
    }
    fn description(&self, _input: &GetGoalInput, _options: &DescriptionOptions) -> String {
        "Read the current goal".into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        GET_GOAL_PROMPT.to_string()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.goal.is_available()
    }

    fn is_read_only(&self, _input: &GetGoalInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: GetGoalInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<GetGoalOutput>, ToolError> {
        let Some(snapshot) = ctx.goal.snapshot().await else {
            return Err(ToolError::InvalidInput {
                message: "no active goal in this session".into(),
                error_code: None,
            });
        };
        let output = GetGoalOutput {
            objective: snapshot.objective.text.to_string(),
            status: format!("{:?}", snapshot.status()),
            autonomous_turns_used: snapshot.counters.autonomous_turns,
            autonomous_turns_max: snapshot.budget.max_autonomous_turns.get(),
            total_turns: snapshot.counters.total_turns,
            tokens_used: snapshot.usage.total_tokens(),
            has_contract: snapshot.contract.is_some(),
        };
        Ok(plain_result(output))
    }
}

// ── report_goal_turn ─────────────────────────────────────────────────────────

const REPORT_GOAL_TURN_PROMPT: &str = "Report how this turn advanced the goal. Submit exactly one disposition:
- progress: you made progress; give a short summary and the next step.
- completion_candidate: you believe the goal is complete; list each requirement with whether it is satisfied and cite evidence. The runtime verifies before completing — a report never completes on its own.
- blocked_candidate: an external dependency blocks you; name it and what change is required to proceed.
- waiting: you are waiting on an async condition; describe it.
Cite evidence by the ids the runtime issued for your tool results.";

/// A single requirement's claimed result within a completion candidate.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReportRequirement {
    /// The requirement being claimed.
    pub requirement: String,
    /// Whether it is satisfied.
    pub satisfied: bool,
}

/// Worker disposition input for [`ReportGoalTurnTool`], mirroring the closed
/// `GoalTurnDisposition` set (minus the runtime-only `Unreported`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum ReportGoalTurnInput {
    Progress {
        summary: String,
        next_step: String,
        #[serde(default)]
        evidence: Vec<String>,
    },
    Waiting {
        reason: String,
    },
    CompletionCandidate {
        requirements: Vec<ReportRequirement>,
        #[serde(default)]
        evidence: Vec<String>,
    },
    BlockedCandidate {
        dependency: String,
        required_change: String,
        #[serde(default)]
        evidence: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportGoalTurnOutput {
    pub accepted: bool,
}

pub struct ReportGoalTurnTool;

fn evidence_refs(ids: Vec<String>) -> Vec<EvidenceRef> {
    ids.into_iter()
        .map(|id| EvidenceRef {
            evidence_id: EvidenceId::new(id),
            summary: BoundedText::short("cited evidence"),
        })
        .collect()
}

fn to_disposition(input: ReportGoalTurnInput) -> GoalTurnDisposition {
    match input {
        ReportGoalTurnInput::Progress {
            summary,
            next_step,
            evidence,
        } => GoalTurnDisposition::Progress {
            summary: BoundedText::short(summary),
            next_step: BoundedText::short(next_step),
            evidence: evidence_refs(evidence),
        },
        ReportGoalTurnInput::Waiting { reason } => GoalTurnDisposition::Waiting {
            condition: WaitCondition::External {
                description: BoundedText::short(reason),
            },
        },
        ReportGoalTurnInput::CompletionCandidate {
            requirements,
            evidence,
        } => GoalTurnDisposition::CompletionCandidate {
            coverage: RequirementCoverage {
                requirements: requirements
                    .into_iter()
                    .map(|r| RequirementResult {
                        requirement: BoundedText::short(r.requirement),
                        satisfied: r.satisfied,
                        evidence: Vec::new(),
                    })
                    .collect(),
                asserts_complete: true,
            },
            evidence: evidence_refs(evidence),
        },
        ReportGoalTurnInput::BlockedCandidate {
            dependency,
            required_change,
            evidence,
        } => GoalTurnDisposition::BlockedCandidate {
            evidence: BlockerEvidence::Dependency {
                dependency: BoundedText::short(dependency),
                attempted: Vec::new(),
                evidence: evidence_refs(evidence),
                required_change: BoundedText::short(required_change),
            },
        },
    }
}

#[async_trait::async_trait]
impl Tool for ReportGoalTurnTool {
    type Input = ReportGoalTurnInput;
    coco_tool_runtime::impl_runtime_schema!(ReportGoalTurnInput);
    type Output = ReportGoalTurnOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ReportGoalTurn)
    }
    fn name(&self) -> &str {
        ToolName::ReportGoalTurn.as_str()
    }
    fn description(&self, _input: &ReportGoalTurnInput, _options: &DescriptionOptions) -> String {
        "Report this turn's disposition toward the goal".into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        REPORT_GOAL_TURN_PROMPT.to_string()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.goal.is_available()
    }

    /// Recording a disposition is a control-plane signal, not a workspace
    /// mutation — safe to run alongside read-only tools.
    fn is_read_only(&self, _input: &ReportGoalTurnInput) -> bool {
        true
    }

    async fn execute(
        &self,
        input: ReportGoalTurnInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<ReportGoalTurnOutput>, ToolError> {
        let disposition = to_disposition(input);
        ctx.goal
            .report_turn(disposition)
            .await
            .map_err(|message| ToolError::InvalidInput {
                message,
                error_code: None,
            })?;
        Ok(plain_result(ReportGoalTurnOutput { accepted: true }))
    }

    fn render_for_model(&self, _out: &ReportGoalTurnOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: "Turn disposition recorded. The runtime evaluates completion at turn end.".into(),
            provider_options: None,
        }]
    }
}

// ── create_goal ──────────────────────────────────────────────────────────────

const CREATE_GOAL_PROMPT: &str = "Create a persistent goal the runtime will pursue autonomously across turns until it is complete or blocked. Only use when the user explicitly asked you to set a goal.";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CreateGoalInput {
    /// The durable objective to pursue.
    pub objective: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGoalOutput {
    pub created: bool,
}

pub struct CreateGoalTool;

#[async_trait::async_trait]
impl Tool for CreateGoalTool {
    type Input = CreateGoalInput;
    coco_tool_runtime::impl_runtime_schema!(CreateGoalInput);
    type Output = CreateGoalOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::CreateGoal)
    }
    fn name(&self) -> &str {
        ToolName::CreateGoal.as_str()
    }
    fn description(&self, _input: &CreateGoalInput, _options: &DescriptionOptions) -> String {
        "Create a persistent autonomous goal".into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        CREATE_GOAL_PROMPT.to_string()
    }

    /// Not part of the ambient tool list (§12.1): goal creation is a user
    /// control-plane action (`/goal`). Hidden until an explicit-request signal
    /// exposes it.
    fn is_enabled(&self, _ctx: &ToolUseContext) -> bool {
        false
    }

    async fn execute(
        &self,
        input: CreateGoalInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<CreateGoalOutput>, ToolError> {
        ctx.goal
            .create_goal(GoalCreateRequest {
                objective: input.objective,
            })
            .await
            .map_err(|message| ToolError::InvalidInput {
                message,
                error_code: None,
            })?;
        Ok(plain_result(CreateGoalOutput { created: true }))
    }
}

fn plain_result<T>(data: T) -> ToolResult<T> {
    ToolResult {
        data,
        new_messages: vec![],
        app_state_patch: None,
        permission_updates: Vec::new(),
        display_data: None,
    }
}
