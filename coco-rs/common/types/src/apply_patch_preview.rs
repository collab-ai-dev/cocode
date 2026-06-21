use serde::Deserialize;
use serde::Serialize;

use crate::ExitPlanModeOutcome;

/// UI-only side channel for bounded display data produced by tools.
///
/// This data is for transcript/rendering surfaces only. Provider history and
/// model-visible tool output must continue to use `ToolResultMessage.message`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ToolDisplayData {
    ApplyPatchPreview(ApplyPatchPreview),
    /// Structured answers for a completed AskUserQuestion exchange, rendered as
    /// a styled transcript cell (mirrors codex `RequestUserInputResultCell`)
    /// instead of the raw model-facing prose.
    AskUserQuestionResult(AskUserQuestionResult),
    /// Structured UI payload for ExitPlanMode result rendering. The model still
    /// sees the prose rendered from the tool result data.
    ExitPlanModeResult(ExitPlanModeResult),
}

/// Per-question answers for a completed AskUserQuestion call. Built by the tool
/// from the spliced `answers`/`annotations` envelope; the model still sees the
/// prose in `ToolResultMessage.message`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionResult {
    pub questions: Vec<AskUserQuestionAnswered>,
}

/// One answered (or unanswered) question in an [`AskUserQuestionResult`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionAnswered {
    /// The question text.
    pub question: String,
    /// Selected option label(s). Empty ⇒ unanswered.
    pub answers: Vec<String>,
    /// Freeform note (the "Other" composer text / annotation), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// UI-only data needed to render an ExitPlanMode result without reading private
/// tool input/output wire fields.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitPlanModeResult {
    pub outcome: ExitPlanModeOutcome,
    pub plan: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    pub awaiting_leader_approval: bool,
    pub is_agent: bool,
    pub plan_was_edited: bool,
}

impl ExitPlanModeResult {
    pub fn has_implementation_plan(&self) -> bool {
        self.outcome.has_implementation_plan()
    }
}

/// Bounded, structured preview of an `apply_patch` body for UI rendering.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyPatchPreview {
    pub rows: Vec<ApplyPatchPreviewRow>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApplyPatchPreviewRow {
    Header {
        action: ApplyPatchPreviewAction,
        target: String,
    },
    Line {
        sign: ApplyPatchPreviewSign,
        content: String,
    },
    Raw {
        content: String,
    },
    /// Placeholder for rows removed from a bounded preview at this position.
    Omitted {
        #[serde(deserialize_with = "deserialize_non_negative_i64")]
        rows: i64,
    },
}

fn deserialize_non_negative_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let rows = i64::deserialize(deserializer)?;
    if rows < 0 {
        return Err(serde::de::Error::custom(
            "omitted rows must be non-negative",
        ));
    }
    Ok(rows)
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPatchPreviewAction {
    Add,
    Delete,
    Update,
}

impl ApplyPatchPreviewAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Delete => "delete",
            Self::Update => "update",
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPatchPreviewSign {
    Added,
    Removed,
    Context,
}

impl ApplyPatchPreviewSign {
    pub const fn as_char(self) -> char {
        match self {
            Self::Added => '+',
            Self::Removed => '-',
            Self::Context => ' ',
        }
    }
}

#[cfg(test)]
#[path = "apply_patch_preview.test.rs"]
mod tests;
