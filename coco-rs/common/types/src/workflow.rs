//! Advisory sizing for dynamic workflows.
//!
//! A workflow script decides for itself how many subagents to spawn; nothing in
//! the runtime counts `agent()` calls against this value. The guideline is a
//! *rhetorical* control surface: it renders to one English sentence appended to
//! the Workflow tool's description, where the model reads it as guidance.
//! Keeping it advisory is deliberate — a hard cap would break the scripts the
//! tool prose teaches (loop-until-dry, judge panels) whenever a user's ceiling
//! happened to sit below what the task needed.

use serde::Deserialize;
use serde::Serialize;

/// How large a dynamic workflow should be allowed to grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSizeGuideline {
    /// No sentence is emitted at all. Deliberately not "a very large number":
    /// any number still biases the model downward, whereas saying nothing is
    /// genuinely neutral.
    Unrestricted,
    Small,
    Medium,
    Large,
}

impl WorkflowSizeGuideline {
    /// Applied when nothing configures a guideline. `Medium` is the largest
    /// size whose agent count fits inside one concurrency window, so a
    /// default-config workflow never queues against itself.
    pub const DEFAULT: Self = Self::Medium;

    /// The agent count the model is told to stay under. `None` for
    /// [`Self::Unrestricted`], which carries no cap sentence.
    pub const fn agent_cap(self) -> Option<i32> {
        match self {
            Self::Unrestricted => None,
            Self::Small => Some(5),
            Self::Medium => Some(15),
            Self::Large => Some(50),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unrestricted => "unrestricted",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    /// The size named alongside its cap: `"medium — keep workflows under 15
    /// agents"`. A capless size renders as its bare name.
    pub fn describe_with_cap(self) -> String {
        match self.agent_cap() {
            Some(cap) => format!("{} — keep workflows under {cap} agents", self.as_str()),
            None => self.as_str().to_string(),
        }
    }
}

/// The caveat every guideline sentence ends with. Named because both the tool
/// description and the mid-session change notice must say the same thing — a
/// guideline the model believes is a hard limit in one place and advice in the
/// other is worse than either reading alone.
pub const WORKFLOW_SIZE_GUIDELINE_CAVEAT: &str = "This is a guideline, not a hard limit — follow it unless the user's prompt calls for a \
     different scale.";

/// A guideline plus the provenance that decides how much authority it is
/// presented with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ResolvedWorkflowSize {
    pub size: WorkflowSizeGuideline,
    /// True when nothing configured a guideline and [`WorkflowSizeGuideline::DEFAULT`]
    /// is standing in.
    pub is_default: bool,
}

impl Default for ResolvedWorkflowSize {
    fn default() -> Self {
        Self::resolve(None)
    }
}

impl ResolvedWorkflowSize {
    /// Fold a configured value (`None` ⇒ nobody set one) into the pair the
    /// prose generators read.
    pub const fn resolve(configured: Option<WorkflowSizeGuideline>) -> Self {
        match configured {
            Some(size) => Self {
                size,
                is_default: false,
            },
            None => Self {
                size: WorkflowSizeGuideline::DEFAULT,
                is_default: true,
            },
        }
    }

    /// The sentence appended to the Workflow tool's description. Empty for
    /// [`WorkflowSizeGuideline::Unrestricted`] — no guideline, no sentence.
    ///
    /// The default and the explicit case are worded differently on purpose. The
    /// closing "you can change it" pointer is a licence to argue with the
    /// constraint: useful when nobody chose anything, but an invitation to
    /// override a decision the user already made when they did.
    pub fn tool_description_sentence(&self) -> String {
        if self.size == WorkflowSizeGuideline::Unrestricted {
            return String::new();
        }
        let lead = if self.is_default {
            "This session has the default workflow size guideline:"
        } else {
            "A workflow size guideline is configured for this session:"
        };
        // The pointer names the command that actually exists here. Claude Code
        // sends the model to a `/config` *row*; coco's `/config` is a key/value
        // command, and telling the model to look for a dialog it will never
        // find is worse than not telling it anything.
        let escape_hatch = if self.is_default {
            " The user can raise or remove it with \
             `/config workflowSizeGuideline <unrestricted|small|medium|large>`."
        } else {
            ""
        };
        format!(
            "{lead} {described}. {WORKFLOW_SIZE_GUIDELINE_CAVEAT}{escape_hatch}",
            described = self.size.describe_with_cap(),
        )
    }

    /// The mid-session notice telling the model its guideline moved. Delivered
    /// as a reminder rather than by re-rendering the tool description, which
    /// sits in the request's cached prefix.
    pub fn change_notice(&self) -> String {
        if self.size == WorkflowSizeGuideline::Unrestricted {
            return "Workflow size is now unrestricted — no size guideline applies.".to_string();
        }
        format!(
            "The workflow size guideline for this session changed: {described}. \
             {WORKFLOW_SIZE_GUIDELINE_CAVEAT}",
            described = self.size.describe_with_cap(),
        )
    }
}

#[cfg(test)]
#[path = "workflow.test.rs"]
mod tests;
