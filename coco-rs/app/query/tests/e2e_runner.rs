//! JSON-driven e2e test framework.
//!
//! Each test scenario is a `.json` file in `tests/scenarios/` that defines:
//! - Initial workspace files
//! - A mock LLM response script (multi-turn, with tool calls)
//! - Assertions on final state (response text, file contents, turn count)
//!
//! The framework supports parallel tool calls, workspace templating via
//! `{{workspace}}`, event capture, budget/turn limits, and auto-discovery.
//!
//! Usage:
//! ```bash
//! cargo test -p coco-query --test e2e_runner
//! cargo test -p coco-query --test e2e_runner test_scenario_write_doc
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use coco_query::AgentStreamEvent;
use coco_query::CoreEvent;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::QueryResult;
use coco_query::ServerNotification;
use coco_tool_runtime::ToolRegistry;
use coco_tools::AgentTool;
use coco_tools::BashTool;
use coco_tools::EditTool;
use coco_tools::GlobTool;
use coco_tools::GrepTool;
use coco_tools::ReadTool;
use coco_tools::SendMessageTool;
use coco_tools::SkillTool;
use coco_tools::TeamCreateTool;
use coco_tools::TeamDeleteTool;
use coco_tools::WriteTool;
use coco_types::PermissionMode;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

// ─── JSON scenario types ───

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    workspace: HashMap<String, String>,
    turns: Vec<ScenarioTurn>,
    expect: ScenarioExpect,
    #[serde(default)]
    config: ScenarioConfig,
}

#[derive(Debug, Deserialize)]
struct ScenarioTurn {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ScenarioToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScenarioToolCall {
    tool: String,
    input: Value,
}

#[derive(Debug, Deserialize)]
struct ScenarioExpect {
    turns: Option<i32>,
    min_turns: Option<i32>,
    response_contains: Option<String>,
    #[serde(default)]
    response_not_contains: Option<String>,
    #[serde(default)]
    files: Option<HashMap<String, FileExpect>>,
    // Boolean assertions: true = assert the condition holds
    not_cancelled: Option<bool>,
    not_budget_exhausted: Option<bool>,
    budget_exhausted: Option<bool>,
    cancelled: Option<bool>,
    // Event count assertions (event_type → expected count)
    #[serde(default)]
    event_counts: HashMap<String, i32>,
    // Min event counts (event_type → minimum count)
    #[serde(default)]
    min_event_counts: HashMap<String, i32>,
}

#[derive(Debug, Deserialize)]
struct FileExpect {
    exists: Option<bool>,
    contains: Option<String>,
    #[serde(default)]
    contains_all: Vec<String>,
    not_contains: Option<String>,
    equals: Option<String>,
    line_count: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
struct ScenarioConfig {
    max_turns: Option<i32>,
    #[serde(default)]
    max_tokens: Option<i64>,
    /// Tool set: "core" (default 6 tools) or "extended" (adds Agent, Skill, etc.)
    #[serde(default)]
    tools: Option<String>,
}

// ─── Resolved turn (after template substitution) ───

#[derive(Debug, Clone)]
struct ResolvedTurn {
    text: Option<String>,
    tool_calls: Vec<ResolvedToolCall>,
}

#[derive(Debug, Clone)]
struct ResolvedToolCall {
    tool: String,
    input: Value,
}

// ─── JSON-driven mock LLM ───

/// A mock model that plays back turns defined in a JSON scenario.
struct JsonScriptedMock {
    call_count: AtomicI32,
    turns: Vec<ResolvedTurn>,
}

impl JsonScriptedMock {
    fn new(turns: Vec<ResolvedTurn>) -> Self {
        Self {
            call_count: AtomicI32::new(0),
            turns,
        }
    }

    fn build_response(&self, idx: i32) -> LanguageModelGenerateResult {
        let i = idx as usize;
        if i >= self.turns.len() {
            return make_text_result("(mock: no more scripted turns)", idx);
        }

        let turn = &self.turns[i];
        let has_text = turn.text.is_some();
        let has_tools = !turn.tool_calls.is_empty();

        match (has_text, has_tools) {
            (true, false) | (false, false) => {
                let text = turn.text.clone().unwrap_or_default();
                make_text_result(&text, idx)
            }
            (false, true) => make_tool_result(&turn.tool_calls, idx),
            (true, true) => make_text_and_tool_result(
                turn.text.as_deref().unwrap_or_default(),
                &turn.tool_calls,
                idx,
            ),
        }
    }
}

#[async_trait::async_trait]
impl LanguageModel for JsonScriptedMock {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "json-scripted-mock"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(self.build_response(idx))
    }

    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

// ─── Response builders ───

fn make_text_result(text: &str, _call_idx: i32) -> LanguageModelGenerateResult {
    LanguageModelGenerateResult {
        content: vec![AssistantContentPart::Text(TextPart {
            text: text.to_string(),
            provider_metadata: None,
        })],
        usage: Usage::new(50, 20),
        finish_reason: FinishReason::new(StopReason::EndTurn),
        warnings: vec![],
        provider_metadata: None,
        request: None,
        response: None,
    }
}

fn make_tool_result(tool_calls: &[ResolvedToolCall], call_idx: i32) -> LanguageModelGenerateResult {
    let content = tool_calls
        .iter()
        .enumerate()
        .map(|(i, tc)| {
            AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id: format!("call_{call_idx}_{i}"),
                tool_name: tc.tool.clone(),
                input: tc.input.clone(),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            })
        })
        .collect();

    LanguageModelGenerateResult {
        content,
        usage: Usage::new(50, 20),
        finish_reason: FinishReason::new(StopReason::ToolUse),
        warnings: vec![],
        provider_metadata: None,
        request: None,
        response: None,
    }
}

fn make_text_and_tool_result(
    text: &str,
    tool_calls: &[ResolvedToolCall],
    call_idx: i32,
) -> LanguageModelGenerateResult {
    let mut content = vec![AssistantContentPart::Text(TextPart {
        text: text.to_string(),
        provider_metadata: None,
    })];
    for (i, tc) in tool_calls.iter().enumerate() {
        content.push(AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: format!("call_{call_idx}_{i}"),
            tool_name: tc.tool.clone(),
            input: tc.input.clone(),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        }));
    }

    LanguageModelGenerateResult {
        content,
        usage: Usage::new(50, 20),
        finish_reason: FinishReason::new(StopReason::ToolUse),
        warnings: vec![],
        provider_metadata: None,
        request: None,
        response: None,
    }
}

// ─── Template substitution ───

/// Replace `{{workspace}}` in all string values within a JSON Value.
fn substitute_value(val: &Value, workspace: &str) -> Value {
    match val {
        Value::String(s) => Value::String(s.replace("{{workspace}}", workspace)),
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| substitute_value(v, workspace)).collect())
        }
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), substitute_value(v, workspace)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn resolve_turns(turns: &[ScenarioTurn], workspace: &str) -> Vec<ResolvedTurn> {
    turns
        .iter()
        .map(|turn| ResolvedTurn {
            text: turn
                .text
                .as_ref()
                .map(|t| t.replace("{{workspace}}", workspace)),
            tool_calls: turn
                .tool_calls
                .iter()
                .map(|tc| ResolvedToolCall {
                    tool: tc.tool.clone(),
                    input: substitute_value(&tc.input, workspace),
                })
                .collect(),
        })
        .collect()
}

// ─── Workspace setup ───

fn setup_workspace(scenario: &Scenario, dir: &Path) {
    for (rel_path, content) in &scenario.workspace {
        let full = dir.join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
    }

    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/workspaces")
        .join(&scenario.name);
    if fixture_dir.exists() {
        copy_dir_recursive(&fixture_dir, dir);
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path).unwrap();
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

// ─── Core tools & runner ───

fn core_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(ReadTool));
    registry.register(Arc::new(WriteTool));
    registry.register(Arc::new(EditTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(GrepTool));
    Arc::new(registry)
}

/// Extended tool set: core + Agent, Skill, SendMessage, Team tools.
fn extended_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(ReadTool));
    registry.register(Arc::new(WriteTool));
    registry.register(Arc::new(EditTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(GrepTool));
    registry.register(Arc::new(AgentTool));
    registry.register(Arc::new(SkillTool));
    registry.register(Arc::new(SendMessageTool));
    registry.register(Arc::new(TeamCreateTool));
    registry.register(Arc::new(TeamDeleteTool));
    Arc::new(registry)
}

fn tools_for_config(config: &ScenarioConfig) -> Arc<ToolRegistry> {
    match config.tools.as_deref() {
        Some("extended") => extended_tools(),
        _ => core_tools(),
    }
}

/// Collected events from a scenario run.
struct ScenarioRunResult {
    query: QueryResult,
    events: Vec<CoreEvent>,
}

async fn run_with_events(
    model: Arc<dyn LanguageModel>,
    prompt: &str,
    tools: Arc<ToolRegistry>,
    config_overrides: &ScenarioConfig,
) -> ScenarioRunResult {
    let client = coco_query::test_support::model_runtime_registry(model);
    let cancel = CancellationToken::new();
    let config = QueryEngineConfig {
        model_id: "json-scripted-mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: config_overrides.max_turns.or(Some(10)),
        total_token_budget: config_overrides.max_tokens,
        ..Default::default()
    };
    let engine = QueryEngine::new(config, client, tools, cancel, None);

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<CoreEvent>(256);
    let events_handle = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = event_rx.recv().await {
            events.push(event);
        }
        events
    });

    let query = engine
        .run_with_events(prompt, event_tx, coco_types::TurnId::generate())
        .await
        .expect("scenario engine should not fail");

    let events = events_handle.await.unwrap();
    ScenarioRunResult { query, events }
}

// ─── Event classification ───
//
// Scenario JSON fixtures reference event types by legacy string names (e.g.
// "ToolUseEnd") for backwards compatibility with the pre-CoreEvent test suite.
// This classifier maps CoreEvent variants back to those legacy names so the
// fixture files don't need updating.

fn classify_event(event: &CoreEvent) -> &'static str {
    match event {
        CoreEvent::Protocol(n) => match n {
            ServerNotification::TurnStarted(_) => "TurnStarted",
            ServerNotification::TurnEnded(_) => "TurnEnded",
            ServerNotification::ContextCompacted(_) => "CompactionTriggered",
            ServerNotification::CompactionStarted => "ErrorRecovery",
            ServerNotification::QueueStateChanged { .. } => "CommandsDrained",
            ServerNotification::Error(_) => "BudgetNudge",
            _ => "Other",
        },
        CoreEvent::Stream(s) => match s {
            AgentStreamEvent::TextDelta { .. } => "TextDelta",
            AgentStreamEvent::ThinkingDelta { .. } => "ReasoningDelta",
            AgentStreamEvent::ToolUseQueued { .. } => "ToolUseStart",
            AgentStreamEvent::ToolUseStarted { .. } => "ToolUseStart",
            AgentStreamEvent::ToolUseCompleted { .. } => "ToolUseEnd",
            AgentStreamEvent::McpToolCallBegin { .. } => "ToolUseStart",
            AgentStreamEvent::McpToolCallEnd { .. } => "ToolUseEnd",
        },
        CoreEvent::Tui(_) => "Tui",
    }
}

fn count_events(events: &[CoreEvent]) -> HashMap<&'static str, i32> {
    let mut counts: HashMap<&'static str, i32> = HashMap::new();
    for event in events {
        *counts.entry(classify_event(event)).or_default() += 1;
    }
    counts
}

// ─── Assertions ───

fn assert_scenario(
    expect: &ScenarioExpect,
    result: &QueryResult,
    events: &[CoreEvent],
    workspace: &Path,
) {
    if let Some(expected_turns) = expect.turns {
        assert_eq!(
            result.turns, expected_turns,
            "expected {expected_turns} turns, got {}",
            result.turns
        );
    }

    if let Some(min) = expect.min_turns {
        assert!(
            result.turns >= min,
            "expected at least {min} turns, got {}",
            result.turns
        );
    }

    if let Some(ref needle) = expect.response_contains {
        assert!(
            result.response_text.contains(needle),
            "response should contain {needle:?}, got: {:?}",
            result.response_text
        );
    }

    if let Some(ref needle) = expect.response_not_contains {
        assert!(
            !result.response_text.contains(needle),
            "response should NOT contain {needle:?}, got: {:?}",
            result.response_text
        );
    }

    // Cancelled assertions
    if let Some(true) = expect.not_cancelled {
        assert!(!result.cancelled, "expected not cancelled");
    }
    if let Some(expected) = expect.cancelled {
        assert_eq!(result.cancelled, expected, "cancelled mismatch");
    }

    // Budget assertions
    if let Some(true) = expect.not_budget_exhausted {
        assert!(!result.budget_exhausted, "expected not budget exhausted");
    }
    if let Some(expected) = expect.budget_exhausted {
        assert_eq!(
            result.budget_exhausted, expected,
            "budget_exhausted mismatch"
        );
    }

    // File assertions
    if let Some(ref files) = expect.files {
        for (rel_path, file_expect) in files {
            let full_path = workspace.join(rel_path);
            assert_file(&full_path, file_expect, rel_path);
        }
    }

    // Event count assertions
    if !expect.event_counts.is_empty() || !expect.min_event_counts.is_empty() {
        let actual = count_events(events);

        for (event_type, expected_count) in &expect.event_counts {
            let actual_count = actual.get(event_type.as_str()).copied().unwrap_or(0);
            assert_eq!(
                actual_count, *expected_count,
                "event {event_type}: expected {expected_count}, got {actual_count}"
            );
        }

        for (event_type, min_count) in &expect.min_event_counts {
            let actual_count = actual.get(event_type.as_str()).copied().unwrap_or(0);
            assert!(
                actual_count >= *min_count,
                "event {event_type}: expected at least {min_count}, got {actual_count}"
            );
        }
    }
}

fn assert_file(path: &Path, expect: &FileExpect, rel_path: &str) {
    if let Some(should_exist) = expect.exists {
        assert_eq!(
            path.exists(),
            should_exist,
            "file {rel_path}: exists={}, expected={should_exist}",
            path.exists()
        );
        if !should_exist {
            return;
        }
    }

    let content =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {rel_path}: {e}"));

    if let Some(ref needle) = expect.contains {
        assert!(
            content.contains(needle),
            "file {rel_path} should contain {needle:?}, got:\n{content}"
        );
    }

    for needle in &expect.contains_all {
        assert!(
            content.contains(needle),
            "file {rel_path} should contain {needle:?}, got:\n{content}"
        );
    }

    if let Some(ref needle) = expect.not_contains {
        assert!(
            !content.contains(needle),
            "file {rel_path} should NOT contain {needle:?}, got:\n{content}"
        );
    }

    if let Some(ref expected) = expect.equals {
        assert_eq!(&content, expected, "file {rel_path} content mismatch");
    }

    if let Some(expected_lines) = expect.line_count {
        let actual = content.lines().count();
        assert_eq!(
            actual, expected_lines,
            "file {rel_path}: expected {expected_lines} lines, got {actual}"
        );
    }
}

// ─── Scenario runner ───

async fn run_scenario(path: &Path) {
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read scenario {}: {e}", path.display()));
    let scenario: Scenario = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("failed to parse scenario {}: {e}", path.display()));

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    setup_workspace(&scenario, dir.path());

    let resolved = resolve_turns(&scenario.turns, ws);
    let prompt = scenario.prompt.as_deref().unwrap_or("run scenario");

    let model: Arc<dyn LanguageModel> = Arc::new(JsonScriptedMock::new(resolved));
    let tools = tools_for_config(&scenario.config);
    let run = run_with_events(model, prompt, tools, &scenario.config).await;

    assert_scenario(&scenario.expect, &run.query, &run.events, dir.path());
}

fn scenario_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/scenarios")
        .join(name)
}

// ─── Individual scenario tests ───

macro_rules! scenario_test {
    ($name:ident, $file:expr) => {
        #[tokio::test]
        async fn $name() {
            run_scenario(&scenario_path($file)).await;
        }
    };
}

// Original 7 scenarios
scenario_test!(test_scenario_write_doc, "write_doc.json");
scenario_test!(test_scenario_edit_doc, "edit_doc.json");
scenario_test!(
    test_scenario_write_edit_read_chain,
    "write_edit_read_chain.json"
);
scenario_test!(test_scenario_parallel_reads, "parallel_reads.json");
scenario_test!(
    test_scenario_multi_file_parallel_write,
    "multi_file_parallel_write.json"
);
scenario_test!(test_scenario_glob_grep_search, "glob_grep_search.json");
scenario_test!(test_scenario_bash_echo, "bash_echo.json");

// New scenarios
scenario_test!(
    test_scenario_text_with_tool_calls,
    "text_with_tool_calls.json"
);
scenario_test!(test_scenario_nested_dir_write, "nested_dir_write.json");
scenario_test!(
    test_scenario_multi_edit_same_file,
    "multi_edit_same_file.json"
);
scenario_test!(
    test_scenario_tool_error_read_missing,
    "tool_error_read_missing.json"
);
scenario_test!(
    test_scenario_large_parallel_batch,
    "large_parallel_batch.json"
);
scenario_test!(test_scenario_parallel_search, "parallel_search.json");
scenario_test!(test_scenario_budget_exhaustion, "budget_exhaustion.json");
scenario_test!(test_scenario_max_turns_limit, "max_turns_limit.json");
scenario_test!(test_scenario_bash_multi_command, "bash_multi_command.json");

// Agent & Skill scenarios
scenario_test!(test_scenario_agent_spawn_basic, "agent_spawn_basic.json");
scenario_test!(
    test_scenario_agent_spawn_then_file_ops,
    "agent_spawn_then_file_ops.json"
);
scenario_test!(
    test_scenario_agent_spawn_invalid_prompt,
    "agent_spawn_invalid_prompt.json"
);
scenario_test!(test_scenario_skill_invoke_basic, "skill_invoke_basic.json");
scenario_test!(
    test_scenario_skill_invoke_with_args,
    "skill_invoke_with_args.json"
);
scenario_test!(
    test_scenario_skill_invoke_invalid_empty,
    "skill_invoke_invalid_empty.json"
);
scenario_test!(
    test_scenario_skill_then_file_chain,
    "skill_then_file_chain.json"
);
scenario_test!(test_scenario_agent_skill_mixed, "agent_skill_mixed.json");
scenario_test!(
    test_scenario_team_create_send_message,
    "team_create_send_message.json"
);

// Compact scenarios
scenario_test!(test_scenario_compact_no_trigger, "compact_no_trigger.json");

// ─── Discovery test: runs ALL scenarios in the directory ───

#[tokio::test]
async fn test_all_scenarios() {
    let scenarios_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    let mut count = 0;

    for entry in std::fs::read_dir(&scenarios_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            println!("=== Running scenario: {} ===", path.display());
            run_scenario(&path).await;
            count += 1;
        }
    }

    assert!(
        count > 0,
        "no scenarios found in {}",
        scenarios_dir.display()
    );
    println!("=== All {count} scenarios passed ===");
}
