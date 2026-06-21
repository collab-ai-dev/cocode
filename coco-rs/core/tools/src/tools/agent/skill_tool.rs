//! `SkillTool` — execute a skill (slash command) within the main conversation.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::SkillGateContext;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Typed input for [`SkillTool`].
///
/// `skill` is required, `args` is optional. (No `#[derive(Default)]`: `skill` is a required
/// non-`Option` field.)
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SkillInput {
    /// The skill name. E.g., "commit", "review-pr", or "pdf"
    pub skill: String,
    /// Optional arguments for the skill
    #[serde(default)]
    pub args: Option<String>,
}

/// Typed output for [`SkillTool`]. Tagged union of `inline` (resolved
/// prompt fed back into the agent's history) and `forked` (child agent
/// ran in its own session, output aggregated).
///
/// Wire field names preserved (`commandName`, `agentId`) via `rename` for cross-runtime
/// transcript compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SkillOutput {
    /// Inline path — the resolved prompt was spliced into history via
    /// `new_messages`; the renderer emits `Launching skill: {name}`.
    Inline {
        #[serde(default)]
        success: bool,
        #[serde(rename = "commandName", default)]
        command_name: String,
        #[serde(default)]
        summary: String,
        /// Raw JSON message Values handed to the runtime's
        /// `Value → Message` adapter. Kept as `Vec<Value>` because the
        /// concrete `Message` type lives outside this module.
        #[serde(default)]
        new_messages: Vec<Value>,
    },
    /// Forked path — the skill ran as a subagent; the renderer emits
    /// `Skill "{name}" completed (forked execution).\n\nResult:\n...`.
    Forked {
        #[serde(default)]
        success: bool,
        #[serde(rename = "commandName", default)]
        command_name: String,
        #[serde(rename = "agentId", default)]
        agent_id: String,
        #[serde(default)]
        result: String,
    },
}

pub struct SkillTool;

#[async_trait::async_trait]
impl Tool for SkillTool {
    type Input = SkillInput;
    coco_tool_runtime::impl_runtime_schema!(SkillInput);
    type Output = SkillOutput;

    fn to_auto_classifier_input(&self, input: &SkillInput) -> Option<String> {
        Some(match &input.args {
            Some(args) => format!("{} {args}", input.skill),
            None => input.skill.clone(),
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Skill)
    }
    fn name(&self) -> &str {
        ToolName::Skill.as_str()
    }
    /// Short UI label.
    fn search_hint(&self) -> Option<&str> {
        Some("invoke a slash-command skill")
    }
    fn description(&self, input: &SkillInput, _options: &DescriptionOptions) -> String {
        format!("Execute skill: {}", input.skill)
    }

    /// Full model-facing tool description. The `<command-name>` literal
    /// matches the XML tag used in skill injection.
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        "Execute a skill within the main conversation\n\
         \n\
         When users ask you to perform tasks, check if any of the available skills match. \
         Skills provide specialized capabilities and domain knowledge.\n\
         \n\
         When users reference a \"slash command\" or \"/<something>\" (e.g., \"/commit\", \
         \"/review-pr\"), they are referring to a skill. Use this tool to invoke it.\n\
         \n\
         How to invoke:\n\
         - Use this tool with the skill name and optional arguments\n\
         - Examples:\n\
         \u{20}\u{20}- `skill: \"pdf\"` - invoke the pdf skill\n\
         \u{20}\u{20}- `skill: \"commit\", args: \"-m 'Fix bug'\"` - invoke with arguments\n\
         \u{20}\u{20}- `skill: \"review-pr\", args: \"123\"` - invoke with arguments\n\
         \u{20}\u{20}- `skill: \"ms-office-suite:pdf\"` - invoke using fully qualified name\n\
         \n\
         Important:\n\
         - Available skills are listed in system-reminder messages in the conversation\n\
         - When a skill matches the user's request, this is a BLOCKING REQUIREMENT: invoke the \
         relevant Skill tool BEFORE generating any other response about the task\n\
         - NEVER mention a skill without actually calling this tool\n\
         - Do not invoke a skill that is already running\n\
         - Do not use this tool for built-in CLI commands (like /help, /clear, etc.)\n\
         - If you see a <command-name> tag in the current conversation turn, the skill has \
         ALREADY been loaded - follow the instructions directly instead of calling this tool again\n"
            .to_string()
    }

    /// Render the skill envelope.
    fn render_for_model(&self, out: &SkillOutput) -> Vec<ToolResultContentPart> {
        let text = match out {
            SkillOutput::Inline { command_name, .. } => {
                format!("Launching skill: {command_name}")
            }
            SkillOutput::Forked {
                command_name,
                result,
                ..
            } => format!(
                "Skill \"{command_name}\" completed (forked execution).\n\nResult:\n{result}"
            ),
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: SkillInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<SkillOutput>, ToolError> {
        if input.skill.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "skill name is required".into(),
                error_code: None,
            });
        }

        let skill_name = input.skill.as_str();
        let args = input.args.as_deref().unwrap_or_default();

        // Skills route through the dedicated `SkillHandle` (Phase 7 split)
        // rather than `AgentHandle` — skills are a different runtime
        // concept. Forward parent's Layer 1 + 2 so a fork-mode skill
        // subagent inherits the same gate set; inline expansion ignores it.
        let inherit = coco_tool_runtime::SubagentInheritance {
            session_id: ctx.session_id_for_history.clone().unwrap_or_default(),
            permission_mode: ctx.permission_context.mode,
            features: Some(ctx.features.clone()),
            tool_overrides: Some(ctx.tool_overrides.clone()),
            active_shell_tool: ctx.active_shell_tool,
            parent_tool_filter: Some(ctx.tool_filter.clone()),
        };
        // `gate` carries the inputs `QuerySkillRuntime` needs to enforce
        // the 4-state Skill tool gate. With default-empty `skill_overrides`
        // tiers the gate short-circuits to `On` so PR2 introduces no
        // observable behavior change.
        //
        // The handle resolves the skill (canonical name + aliases)
        // and tests every candidate against `typed_slashes_in_turn`
        // — `commit-fast → commit` aliases bypass the gate
        // correctly even though the SkillTool only sees the
        // canonical name.
        let gate = SkillGateContext {
            overrides: ctx.skill_overrides.clone(),
            typed_slashes_in_turn: ctx.typed_slashes_in_turn(),
        };
        let result = ctx
            .skill
            .invoke_skill(skill_name, args, inherit, gate)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to resolve skill '{skill_name}': {e}"),
                display_data: None,
                source: None,
            })?;

        // Pull `permission_updates` off the inline variant so the
        // skill's `allowed-tools` frontmatter folds into the running
        // session config via the executor's `PermissionRuleHandle`.
        // Fork variant has no return-channel updates — those were
        // applied at dispatch time via `AgentQueryConfig.extra_permission_rules`.
        let (data, permission_updates) = match result {
            coco_tool_runtime::SkillInvocationResult::Inline {
                summary,
                new_messages,
                permission_updates,
            } => {
                let new_messages_value: Vec<Value> = new_messages
                    .iter()
                    .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
                    .collect();
                (
                    SkillOutput::Inline {
                        success: true,
                        command_name: skill_name.to_string(),
                        summary,
                        new_messages: new_messages_value,
                    },
                    permission_updates,
                )
            }
            coco_tool_runtime::SkillInvocationResult::Forked { agent_id, output } => (
                SkillOutput::Forked {
                    success: true,
                    command_name: skill_name.to_string(),
                    agent_id,
                    result: output,
                },
                Vec::new(),
            ),
        };

        Ok(ToolResult {
            data,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates,
            display_data: None,
        })
    }
}
