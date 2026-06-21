use std::sync::Arc;
use std::time::Duration;

use coco_hooks::HookEvaluationResult;
use coco_query::QueryEngineConfig;
use coco_query::hook_llm::HookAgentRunRequest;
use coco_query::hook_llm::HookAgentRunner;
use coco_tool_runtime::ToolRegistry;
use coco_types::ToolId;
use coco_types::ToolName;
use tokio_util::sync::CancellationToken;

use crate::session_runtime::SessionRuntime;

const MAX_AGENT_HOOK_TURNS: i32 = 50;

/// System prompt for the Stop-hook (LLM-judge) agent. Replaces the main
/// session prompt for the scoped child engine. The conversation transcript
/// path travels in the Stop hook input JSON that becomes the agent's user
/// prompt, so the agent can `Read` it to inspect history.
const HOOK_AGENT_SYSTEM_PROMPT: &str = "You are verifying a stop condition in Claude Code. \
Your task is to verify that the agent completed the given condition. The conversation \
transcript path is provided in the hook input — you can Read that file to analyze the \
conversation history if needed.

Use the available tools to inspect the codebase and verify the condition. Use as few steps \
as possible — be efficient and direct.

When done, return your result using the StructuredOutput tool with:
- ok: true if the condition is met
- ok: false with reason if the condition is not met";

pub struct SessionRuntimeHookAgentRunner {
    runtime: Arc<SessionRuntime>,
}

impl std::fmt::Debug for SessionRuntimeHookAgentRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRuntimeHookAgentRunner").finish()
    }
}

impl SessionRuntimeHookAgentRunner {
    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl HookAgentRunner for SessionRuntimeHookAgentRunner {
    async fn run(&self, request: HookAgentRunRequest) -> HookEvaluationResult {
        let result =
            tokio::time::timeout(request.timeout, run_agent(self.runtime.clone(), request)).await;
        match result {
            Err(_elapsed) => HookEvaluationResult::Cancelled,
            Ok(Ok(output)) => output,
            Ok(Err(error)) => HookEvaluationResult::NonBlockingError { error },
        }
    }
}

async fn run_agent(
    runtime: Arc<SessionRuntime>,
    request: HookAgentRunRequest,
) -> Result<HookEvaluationResult, String> {
    let tools = scoped_tool_registry(&runtime)?;
    let hooks = scoped_hook_registry()?;
    let mut config = runtime.current_engine_config().await;
    configure_hook_agent(&mut config, &request);

    let cancel = CancellationToken::new();
    let engine = runtime
        .build_engine_from_config_with_registries(config, cancel, tools, Some(hooks))
        .await
        .with_model_runtime_source(request.model_source.clone());

    let query_result = engine
        .run(&request.prompt)
        .await
        .map_err(|e| format!("hook agent engine run: {e}"))?;

    parse_structured_output(query_result.structured_output)
}

fn scoped_tool_registry(runtime: &SessionRuntime) -> Result<Arc<ToolRegistry>, String> {
    let registry = Arc::new(ToolRegistry::new());
    for tool in runtime.registered_tools() {
        // Withhold tools a Stop-hook judge must not use — spawning
        // subagents, entering/exiting plan mode, asking the user, or
        // stopping tasks — to keep the verifier from steering the main
        // session.
        if is_agent_hook_disallowed_tool(&tool.id()) {
            continue;
        }
        registry.register(tool);
    }
    coco_tools::register_structured_output_tool(&registry, hook_agent_schema())?;
    Ok(registry)
}

/// Builtin tools withheld from a Stop-hook agent. The Agent tool is
/// always withheld. coco-rs has no Workflow tool.
fn is_agent_hook_disallowed_tool(id: &ToolId) -> bool {
    matches!(
        id,
        ToolId::Builtin(
            ToolName::TaskOutput
                | ToolName::ExitPlanMode
                | ToolName::EnterPlanMode
                | ToolName::Agent
                | ToolName::AskUserQuestion
                | ToolName::TaskStop
        )
    )
}

fn scoped_hook_registry() -> Result<Arc<coco_hooks::HookRegistry>, String> {
    let registry = Arc::new(coco_hooks::HookRegistry::new());
    registry
        .register_function_hook(
            format!("hook-agent-structured-output-{}", uuid::Uuid::new_v4()),
            coco_types::HookEventType::Stop,
            None,
            Duration::from_millis(5_000),
            Arc::new(coco_query::structured_output_enforcement::StructuredOutputEnforcement),
            format!(
                "You MUST call the {} tool to complete this request. Call this tool now.",
                coco_types::ToolName::StructuredOutput.as_str()
            ),
        )
        .map_err(|e| format!("failed to register hook-agent StructuredOutput Stop hook: {e}"))?;
    Ok(registry)
}

fn configure_hook_agent(config: &mut QueryEngineConfig, request: &HookAgentRunRequest) {
    config.model_id = request.model_id.clone();
    config.permission_mode = coco_types::PermissionMode::Default;
    config.max_turns = Some(MAX_AGENT_HOOK_TURNS);
    config.total_token_budget = None;
    config.streaming_tool_execution = false;
    config.is_non_interactive = true;
    config.avoid_permission_prompts = true;
    // Verifier framing replaces the inherited main-session prompt.
    config.system_prompt = Some(HOOK_AGENT_SYSTEM_PROMPT.to_string());
    // Disable thinking for the verifier; otherwise the child inherits
    // the user's extended-thinking budget from the cloned session config.
    config.thinking_level = None;
    config.query_source_override = Some(coco_types::ForkLabel::HookAgent.as_str().to_string());
    config.fork_label = Some(coco_types::ForkLabel::HookAgent);
    config.session_id = format!("hook-agent-{}", uuid::Uuid::new_v4());
}

fn hook_agent_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["ok"],
        "properties": {
            "ok": { "type": "boolean" },
            "reason": { "type": "string" }
        }
    })
}

fn parse_structured_output(
    structured_output: Option<serde_json::Value>,
) -> Result<HookEvaluationResult, String> {
    let Some(value) = structured_output else {
        return Ok(HookEvaluationResult::Cancelled);
    };
    let Some(ok) = value.get("ok").and_then(serde_json::Value::as_bool) else {
        return Err("hook agent StructuredOutput missing boolean `ok`".to_string());
    };
    if ok {
        return Ok(HookEvaluationResult::Ok);
    }
    // Keep the `Agent hook condition was not met: ${reason}` prefix for
    // blocking feedback; drop the trailing `: ` when no reason was given.
    let reason = match value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(reason) => format!("Agent hook condition was not met: {reason}"),
        None => "Agent hook condition was not met".to_string(),
    };
    Ok(HookEvaluationResult::Blocking { reason })
}

pub async fn install(runtime: Arc<SessionRuntime>) {
    let runner: coco_query::hook_llm::HookAgentRunnerRef =
        Arc::new(SessionRuntimeHookAgentRunner::new(runtime.clone()));
    runtime.attach_hook_agent_runner(runner).await;
}

#[cfg(test)]
#[path = "hook_agent_runner.test.rs"]
mod tests;
