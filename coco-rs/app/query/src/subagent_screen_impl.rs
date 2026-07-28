//! Auto-mode screening for subagent dispatches that never reach the tool
//! pipeline — see [`coco_tool_runtime::subagent_screen`] for why the seam
//! exists.
//!
//! The implementation rebuilds the equivalent `Agent(prompt, subagent_type)`
//! tool call and runs it through the very same
//! [`coco_permissions::can_use_tool_in_auto_mode`] a real `Agent` call hits.
//! That sharing is the design: it is what stops the two dispatch paths from
//! drifting apart again.

use std::sync::Arc;

use coco_permissions::AutoModeRules;
use coco_tool_runtime::SubagentDispatch;
use coco_tool_runtime::SubagentDispatchScreen;
use coco_tool_runtime::SubagentDispatchVerdict;
use coco_tool_runtime::ToolRegistry;
use coco_types::PermissionDecision;
use coco_types::PermissionMode;
use coco_types::ToolName;

use coco_inference::ModelRuntimeRegistry;

/// Cap on the serialized `agent({schema})` text handed to the classifier.
///
/// The schema is appended to the classifier's *prompt*, so it is prompt text,
/// not data: an unbounded one is both a dilution attack (enough JSON pushes the
/// dispatch being judged out of the classifier's effective attention) and a
/// direct injection channel (`description` strings are free-form English). ~1k
/// tokens is where the classifier's input stays dominated by what it is meant
/// to judge.
const MAX_CLASSIFIER_SCHEMA_CHARS: usize = 4096;

pub(crate) struct AutoModeSubagentScreen {
    model_runtimes: Arc<ModelRuntimeRegistry>,
    usage_accounting: Option<crate::usage_accounting::UsageAccounting>,
    auto_mode_rules: AutoModeRules,
    tools: Arc<ToolRegistry>,
    /// Denial state for dispatch screening only, owned rather than shared.
    ///
    /// Deliberately NOT the turn's tracker: its consecutive-denial streak
    /// escalates auto mode into an interactive prompt, and a workflow runs
    /// detached with nobody to prompt. Letting a background fan-out drive the
    /// session's escalation would be cross-contamination.
    denial_tracker: Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>,
}

impl AutoModeSubagentScreen {
    pub(crate) fn new(
        model_runtimes: Arc<ModelRuntimeRegistry>,
        usage_accounting: Option<crate::usage_accounting::UsageAccounting>,
        auto_mode_rules: AutoModeRules,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            model_runtimes,
            usage_accounting,
            auto_mode_rules,
            tools,
            denial_tracker: Arc::new(tokio::sync::Mutex::new(
                coco_permissions::DenialTracker::new(),
            )),
        }
    }
}

/// Render the dispatch as the `Agent` tool input it stands for. Any divergence
/// here is a divergence in what the classifier judges, so the field names match
/// `AgentTool`'s wire schema.
fn agent_tool_input(dispatch: &SubagentDispatch<'_>) -> serde_json::Value {
    let mut input = serde_json::json!({ "prompt": dispatch.prompt });
    if let Some(agent_type) = dispatch.subagent_type
        && let serde_json::Value::Object(map) = &mut input
    {
        map.insert(
            "subagent_type".to_string(),
            serde_json::Value::String(agent_type.to_string()),
        );
    }
    input
}

/// Append the requested output schema to the judged prompt, or refuse.
///
/// Fails **closed**: a schema too large or too weird to serialize cannot be
/// shown to the classifier, and dispatching something the classifier was not
/// allowed to see defeats the screen.
fn prompt_with_schema(dispatch: &SubagentDispatch<'_>) -> Result<String, String> {
    let Some(schema) = dispatch.output_schema else {
        return Ok(dispatch.prompt.to_string());
    };
    let Ok(rendered) = serde_json::to_string(schema) else {
        return Err("output schema could not be serialized for classification".to_string());
    };
    if rendered.chars().count() > MAX_CLASSIFIER_SCHEMA_CHARS {
        return Err("output schema too large to classify safely".to_string());
    }
    Ok(format!(
        "{prompt}\n\n[output schema]\n{rendered}",
        prompt = dispatch.prompt
    ))
}

#[async_trait::async_trait]
impl SubagentDispatchScreen for AutoModeSubagentScreen {
    async fn screen(&self, dispatch: SubagentDispatch<'_>) -> SubagentDispatchVerdict {
        // Auto mode is the only mode that screens a dispatch. Every other mode
        // either prompts a human (impossible for a detached run) or has already
        // decided to trust the session.
        if dispatch.permission_context.mode != PermissionMode::Auto {
            return SubagentDispatchVerdict::Allow;
        }

        let judged_prompt = match prompt_with_schema(&dispatch) {
            Ok(prompt) => prompt,
            Err(reason) => return SubagentDispatchVerdict::Block { reason },
        };
        let judged = SubagentDispatch {
            prompt: &judged_prompt,
            ..dispatch
        };
        let input = agent_tool_input(&judged);

        let additional_dirs: Vec<String> = judged
            .permission_context
            .additional_dirs
            .keys()
            .cloned()
            .collect();
        let auto_ctx = coco_permissions::AutoModeContext {
            cwd: judged.cwd,
            additional_dirs: &additional_dirs,
            // A dispatch has no interactive fallback: the workflow is detached,
            // so an "ask the user" verdict is a refusal.
            avoid_permission_prompts: true,
        };

        let mut tracker = self.denial_tracker.lock().await;
        let decision = crate::tool_call_preparer::try_classify_in_auto_mode(
            ToolName::Agent.as_str(),
            &input,
            /*is_read_only*/ false,
            /*auto_active*/ true,
            &mut tracker,
            judged.messages,
            &self.model_runtimes,
            self.usage_accounting.clone(),
            &self.auto_mode_rules,
            auto_ctx,
            &self.tools,
        )
        .await;
        drop(tracker);

        match decision {
            // Deny is the ordinary refusal; Abort is a refusal too (the turn is
            // going away, so nothing should be spawned into it).
            Some(
                PermissionDecision::Deny { message, .. }
                | PermissionDecision::Abort { message, .. },
            ) => SubagentDispatchVerdict::Block { reason: message },
            // `Ask` should be unreachable — a detached dispatch sets
            // `avoid_permission_prompts`, which turns every would-be prompt into
            // a Deny — and `None` means the classifier declined to decide, which
            // is not a block.
            Some(PermissionDecision::Allow { .. } | PermissionDecision::Ask { .. }) | None => {
                SubagentDispatchVerdict::Allow
            }
        }
    }
}

#[cfg(test)]
#[path = "subagent_screen_impl.test.rs"]
mod tests;
