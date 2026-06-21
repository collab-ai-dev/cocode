//! `TeamCreateTool` and `TeamDeleteTool` — team lifecycle.
//!
//! Grouped here because both have a tiny surface and share the
//! `AgentHandle`-routed dispatch shape.

use coco_messages::ToolResult;
use coco_tool_runtime::CreateTeamRequest;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Full model-facing prompt for [`TeamCreateTool`].
const TEAM_CREATE_PROMPT: &str = r#"# TeamCreate

## When to Use

Use this tool proactively whenever:
- The user explicitly asks to use a team, swarm, or group of agents
- The user mentions wanting agents to work together, coordinate, or collaborate
- A task is complex enough that it would benefit from parallel work by multiple agents (e.g., building a full-stack feature with frontend and backend work, refactoring a codebase while keeping tests passing, implementing a multi-step project with research, planning, and coding phases)

When in doubt about whether a task warrants a team, prefer spawning a team.

## Choosing Agent Types for Teammates

When spawning teammates via the Agent tool, choose the `subagent_type` based on what tools the agent needs for its task. Each agent type has a different set of available tools — match the agent to the work:

- **Read-only agents** (e.g., Explore, Plan) cannot edit or write files. Only assign them research, search, or planning tasks. Never assign them implementation work.
- **Full-capability agents** (e.g., general-purpose) have access to all tools including file editing, writing, and bash. Use these for tasks that require making changes.
- **Custom agents** defined in `.coco/agents/` may have their own tool restrictions. Check their descriptions to understand what they can and cannot do.

Always review the agent type descriptions and their available tools listed in the Agent tool prompt before selecting a `subagent_type` for a teammate.

Create a new team to coordinate multiple agents working on a project. Teams have a 1:1 correspondence with task lists (Team = TaskList).

```
{
  "team_name": "my-project",
  "description": "Working on feature X"
}
```

This creates:
- A team file at `~/.coco/teams/{team-name}/config.json`
- A corresponding task list directory at `~/.coco/tasks/{team-name}/`

## Team Workflow

1. **Create a team** with TeamCreate - this creates both the team and its task list
2. **Create tasks** using the Task tools (TaskCreate, TaskList, etc.) - they automatically use the team's task list
3. **Spawn teammates** using the Agent tool with `team_name` and `name` parameters to create teammates that join the team
4. **Assign tasks** using TaskUpdate with `owner` to give tasks to idle teammates
5. **Teammates work on assigned tasks** and mark them completed via TaskUpdate
6. **Teammates go idle between turns** - after each turn, teammates automatically go idle and send a notification. IMPORTANT: Be patient with idle teammates! Don't comment on their idleness until it actually impacts your work.
7. **Shutdown your team** - when the task is completed, gracefully shut down your teammates via SendMessage with `message: {type: "shutdown_request"}`.

## Task Ownership

Tasks are assigned using TaskUpdate with the `owner` parameter. Any agent can set or change task ownership via TaskUpdate.

## Automatic Message Delivery

**IMPORTANT**: Messages from teammates are automatically delivered to you. You do NOT need to manually check your inbox.

When you spawn teammates:
- They will send you messages when they complete tasks or need help
- These messages appear automatically as new conversation turns (like user messages)
- If you're busy (mid-turn), messages are queued and delivered when your turn ends
- The UI shows a brief notification with the sender's name when messages are waiting

Messages will be delivered automatically.

When reporting on teammate messages, you do NOT need to quote the original message—it's already rendered to the user.

## Teammate Idle State

Teammates go idle after every turn—this is completely normal and expected. A teammate going idle immediately after sending you a message does NOT mean they are done or unavailable. Idle simply means they are waiting for input.

- **Idle teammates can receive messages.** Sending a message to an idle teammate wakes them up and they will process it normally.
- **Idle notifications are automatic.** The system sends an idle notification whenever a teammate's turn ends. You do not need to react to idle notifications unless you want to assign new work or send a follow-up message.
- **Do not treat idle as an error.** A teammate sending a message and then going idle is the normal flow—they sent their message and are now waiting for a response.
- **Peer DM visibility.** When a teammate sends a DM to another teammate, a brief summary is included in their idle notification. This gives you visibility into peer collaboration without the full message content. You do not need to respond to these summaries — they are informational.

## Discovering Team Members

Teammates can read the team config file to discover other team members:
- **Team config location**: `~/.coco/teams/{team-name}/config.json`

The config file contains a `members` array with each teammate's:
- `name`: Human-readable name (**always use this** for messaging and task assignment)
- `agentId`: Unique identifier (for reference only - do not use for communication)
- `agentType`: Role/type of the agent

**IMPORTANT**: Always refer to teammates by their NAME (e.g., "team-lead", "researcher", "tester"). Names are used for:
- `to` when sending messages
- Identifying task owners

Example of reading team config:
```
Use the Read tool to read ~/.coco/teams/{team-name}/config.json
```

## Task List Coordination

Teams share a task list that all teammates can access at `~/.coco/tasks/{team-name}/`.

Teammates should:
1. Check TaskList periodically, **especially after completing each task**, to find available work or see newly unblocked tasks
2. Claim unassigned, unblocked tasks with TaskUpdate (set `owner` to your name). **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
3. Create new tasks with `TaskCreate` when identifying additional work
4. Mark tasks as completed with `TaskUpdate` when done, then check TaskList for next work
5. Coordinate with other teammates by reading the task list status
6. If all available tasks are blocked, notify the team lead or help resolve blocking tasks

**IMPORTANT notes for communication with your team**:
- Do not use terminal tools to view your team's activity; always send a message to your teammates (and remember, refer to them by name).
- Your team cannot hear you if you do not use the SendMessage tool. Always send a message to your teammates if you are responding to them.
- Do NOT send structured JSON status messages like `{"type":"idle",...}` or `{"type":"task_completed",...}`. Just communicate in plain text when you need to message teammates.
- Use TaskUpdate to mark tasks completed.
- If you are an agent in the team, the system will automatically send idle notifications to the team lead when you stop."#;

/// Full model-facing prompt for [`TeamDeleteTool`].
const TEAM_DELETE_PROMPT: &str = r#"# TeamDelete

Remove team and task directories when the swarm work is complete.

This operation:
- Removes the team directory (`~/.coco/teams/{team-name}/`)
- Removes the task directory (`~/.coco/tasks/{team-name}/`)
- Clears team context from the current session

**IMPORTANT**: TeamDelete will fail if the team still has active members. Gracefully terminate teammates first, then call TeamDelete after all teammates have shut down.

Use this when all teammates have finished their work and you want to clean up the team resources. The team name is automatically determined from the current session's team context."#;

/// Typed input for [`TeamCreateTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TeamCreateInput {
    /// Name for the new team to create.
    pub team_name: String,
    /// Team description/purpose.
    #[serde(default)]
    pub description: Option<String>,
    /// Type/role of the team lead (e.g., "researcher", "test-runner"). Used for team file and inter-agent coordination.
    #[serde(default)]
    pub agent_type: Option<String>,
}

/// Typed output for [`TeamCreateTool`]. All fields default so partial
/// fixtures (`json!({"team_name": "x"})`) round-trip via the blanket.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamCreateOutput {
    #[serde(default)]
    pub team_name: String,
    #[serde(default)]
    pub lead_agent_id: String,
    #[serde(default)]
    pub task_list_id: String,
}

pub struct TeamCreateTool;

#[async_trait::async_trait]
impl Tool for TeamCreateTool {
    type Input = TeamCreateInput;
    coco_tool_runtime::impl_runtime_schema!(TeamCreateInput);
    type Output = TeamCreateOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TeamCreate)
    }
    fn name(&self) -> &str {
        ToolName::TeamCreate.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::AgentTeams)
    }
    /// Short UI label.
    fn description(&self, _input: &TeamCreateInput, _options: &DescriptionOptions) -> String {
        "Create a new team for coordinating multiple agents".into()
    }

    /// Full model-facing tool description.
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        TEAM_CREATE_PROMPT.to_string()
    }

    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("create a multi-agent swarm team")
    }

    /// Render a compact confirmation. The created `team_name` /
    /// `lead_agent_id` / `task_list_id` are model-visible but the
    /// typical follow-up just needs to know "team is up" — match the
    /// pre-typed default JSON dump shape so consumers can pivot off
    /// `data["team_name"]` etc.
    fn render_for_model(&self, out: &TeamCreateOutput) -> Vec<ToolResultContentPart> {
        let text = serde_json::to_string(out).unwrap_or_default();
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: TeamCreateInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<TeamCreateOutput>, ToolError> {
        if input.team_name.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "team_name is required".into(),
                error_code: None,
            });
        }

        let cwd = match ctx.cwd_override.clone() {
            Some(path) => path,
            None => std::env::current_dir().map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to resolve current directory for TeamCreate: {e}"),
                display_data: None,
                source: None,
            })?,
        };
        let leader_session_id =
            ctx.session_id_for_history
                .clone()
                .ok_or_else(|| ToolError::ExecutionFailed {
                    message: "TeamCreate requires a real leader session id".into(),
                    display_data: None,
                    source: None,
                })?;

        let result = ctx
            .agent
            .create_team(CreateTeamRequest {
                requested_name: input.team_name.clone(),
                leader_agent_id: ctx.agent_id.as_ref().map(ToString::to_string),
                leader_session_id,
                cwd,
                allowed_paths: Vec::new(),
                leader_model: Some(ctx.main_loop_model.clone()),
                task_list_router: ctx.team_task_list_router.clone(),
            })
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: e,
                display_data: None,
                source: None,
            })?;

        Ok(ToolResult {
            data: TeamCreateOutput {
                team_name: result.team_name,
                lead_agent_id: result.lead_agent_id,
                task_list_id: result.task_list_id,
            },
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Typed input for [`TeamDeleteTool`] — no parameters.
///
/// The tool reads the team name from the active session context, not from
/// tool input.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TeamDeleteInput {}

/// Typed output for [`TeamDeleteTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamDeleteOutput {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub message: String,
}

pub struct TeamDeleteTool;

#[async_trait::async_trait]
impl Tool for TeamDeleteTool {
    type Input = TeamDeleteInput;
    coco_tool_runtime::impl_runtime_schema!(TeamDeleteInput);
    type Output = TeamDeleteOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TeamDelete)
    }
    fn name(&self) -> &str {
        ToolName::TeamDelete.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::AgentTeams)
    }
    /// Short UI label.
    fn description(&self, _input: &TeamDeleteInput, _options: &DescriptionOptions) -> String {
        "Clean up team and task directories when the swarm is complete".into()
    }

    /// Full model-facing tool description.
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        TEAM_DELETE_PROMPT.to_string()
    }

    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("disband a swarm team and clean up")
    }

    /// Render the prebuilt `message` field — `success` flag is for
    /// callers that key off `data["success"]`.
    fn render_for_model(&self, out: &TeamDeleteOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.message.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        _input: TeamDeleteInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<TeamDeleteOutput>, ToolError> {
        let result = ctx
            .agent
            .delete_team()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: e,
                display_data: None,
                source: None,
            })?;

        Ok(ToolResult {
            data: TeamDeleteOutput {
                success: true,
                message: result,
            },
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}
