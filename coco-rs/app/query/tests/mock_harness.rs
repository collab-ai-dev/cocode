//! Reusable mock model harness for e2e testing.
//!
//! Provides `MockModelBuilder` to define a sequence of LLM responses
//! (text + tool calls) without writing boilerplate LanguageModel impls.
//!
//! Usage:
//! ```ignore
//! let model = MockModelBuilder::new()
//!     .on_call(0, |_| MockResponse::tool_call("Read", json!({"file_path": "/tmp/x"})))
//!     .on_call(1, |_| MockResponse::text("Done!"))
//!     .build();
//! let result = run_with_mock(model, "read the file", tools).await;
//! assert_eq!(result.turns, 2);
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::sync::Arc;

use coco_inference::{LanguageModel, ModelRuntimeRegistry, PrebuiltLanguageModelSlot};
use coco_query::{QueryEngine, QueryEngineConfig, QueryResult, SessionBootstrap};
use coco_tool_runtime::{
    ToolPermissionBridge, ToolPermissionBridgeRef, ToolPermissionDecision, ToolPermissionRequest,
    ToolPermissionResolution, ToolRegistry,
};
use coco_tools::{
    AgentTool, BashTool, EditTool, EnterPlanModeTool, ExitPlanModeTool, GlobTool, GrepTool,
    ReadTool, WriteTool,
};
use coco_types::{ModelRole, PermissionMode, ToolAppState};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub use coco_test_harness::model::MockModelBuilder;
#[allow(unused_imports)]
pub use coco_test_harness::model::MockResponse;
#[allow(unused_imports)]
pub use coco_test_harness::model::MockToolEmission;
#[allow(unused_imports)]
pub use coco_test_harness::model::ScriptedMock;

// ─── AllowAllPermissionBridge ───

/// Permission bridge that auto-approves every request. Used in
/// integration tests to let the mock model drive tools that return
/// `PermissionDecision::Ask` (e.g. `ExitPlanMode`) without an
/// interactive user. The real `NoOpPermissionBridge` rejects by
/// default, which is wrong for tests whose purpose is to exercise
/// the tool itself.
pub struct AllowAllPermissionBridge;

#[async_trait::async_trait]
impl ToolPermissionBridge for AllowAllPermissionBridge {
    async fn request_permission(
        &self,
        _request: ToolPermissionRequest,
    ) -> Result<ToolPermissionResolution, String> {
        Ok(ToolPermissionResolution {
            decision: ToolPermissionDecision::Approved,
            feedback: None,
            applied_updates: Vec::new(),
            updated_input: None,
            content_blocks: None,
            detail: None,
        })
    }
}

pub fn allow_all_bridge() -> ToolPermissionBridgeRef {
    Arc::new(AllowAllPermissionBridge)
}

// ─── Convenience runners ───

/// Register all 6 core tools.
pub fn core_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(ReadTool));
    registry.register(Arc::new(WriteTool));
    registry.register(Arc::new(EditTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(GrepTool));
    Arc::new(registry)
}

/// Core tools + EnterPlanMode + ExitPlanMode for plan-mode integration
/// tests. Built on top of [`core_tools`] so Read/Write/Grep/etc. are
/// available inside plan mode (model "explores the codebase").
pub fn tools_with_plan_mode() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(ReadTool));
    registry.register(Arc::new(WriteTool));
    registry.register(Arc::new(EditTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(GrepTool));
    registry.register(Arc::new(AgentTool));
    registry.register(Arc::new(EnterPlanModeTool));
    registry.register(Arc::new(ExitPlanModeTool));
    Arc::new(registry)
}

/// Configuration knobs for [`run_plan_mode_turn`]. Wires the shared
/// `ToolAppState` + `config_home` (needed for plan-file I/O) into the
/// engine, and lets the caller drive multi-turn scenarios by threading
/// `final_messages` from the previous turn back in.
pub struct PlanModeTurnParams {
    pub session_id: String,
    pub config_home: std::path::PathBuf,
    pub app_state: Arc<RwLock<ToolAppState>>,
    pub tools: Arc<ToolRegistry>,
    pub plan_role_model: Option<Arc<dyn LanguageModel>>,
    /// Messages from prior turns (plus the new user prompt). When empty
    /// the helper creates a fresh user message from `prompt_if_empty`.
    pub messages: Vec<std::sync::Arc<coco_messages::Message>>,
    /// Fallback prompt when `messages` is empty (first turn case).
    pub prompt_if_empty: String,
    /// Raise this for scenarios that need more than the default 10
    /// tool-iteration budget (e.g. tool_rounds_do_not_advance_cadence).
    /// `None` = unbounded (mirrors `QueryEngineConfig.max_turns`).
    pub max_turns: Option<i32>,
    /// Engine starting mode. Plan-mode tests start here as
    /// [`PermissionMode::Plan`] directly: the engine's plan-mode
    /// reminder snapshots `config.permission_mode` at construction
    /// time, so a model-driven `EnterPlanMode` mid-run wouldn't flip
    /// the reminder on. Matches how real sessions enter plan mode
    /// (Shift+Tab toggle BEFORE `engine.run`).
    pub permission_mode: PermissionMode,
}

impl PlanModeTurnParams {
    /// Convenience: turn 1 from scratch in Plan mode.
    pub fn plan_turn(
        session_id: impl Into<String>,
        config_home: std::path::PathBuf,
        app_state: Arc<RwLock<ToolAppState>>,
        tools: Arc<ToolRegistry>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            config_home,
            app_state,
            tools,
            plan_role_model: None,
            messages: Vec::new(),
            prompt_if_empty: prompt.into(),
            max_turns: Some(20),
            permission_mode: PermissionMode::Plan,
        }
    }

    /// Override the starting mode (used for Reentry tests where the
    /// first run exits back to Default and the second run re-enters
    /// Plan).
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Install a Plan-role model so tests can assert that the engine
    /// swaps clients after live plan-mode entry.
    pub fn with_plan_role_model<M>(mut self, model: Arc<M>) -> Self
    where
        M: LanguageModel + 'static,
    {
        self.plan_role_model = Some(model);
        self
    }

    /// Feed the prior turn's `final_messages` + a new user message into
    /// the next run.
    pub fn next_turn(
        mut self,
        prev_messages: Vec<std::sync::Arc<coco_messages::Message>>,
        prompt: &str,
    ) -> Self {
        self.messages = prev_messages;
        self.messages
            .push(std::sync::Arc::new(coco_messages::create_user_message(
                prompt,
            )));
        self
    }
}

/// Drive one engine run scripted for plan-mode integration tests.
///
/// Wires `app_state` (cross-turn plan cadence + exit flags) and
/// `config_home` (plan-file path resolution), registers the passed
/// tool set, and starts in the caller-specified permission mode.
pub async fn run_plan_mode_turn(
    model: Arc<dyn LanguageModel>,
    params: PlanModeTurnParams,
) -> QueryResult {
    run_plan_mode_turn_with_events(model, params).await.0
}

pub async fn run_plan_mode_turn_with_events(
    model: Arc<dyn LanguageModel>,
    params: PlanModeTurnParams,
) -> (QueryResult, Vec<coco_types::CoreEvent>) {
    let cancel = CancellationToken::new();
    let session_id = match coco_types::SessionId::try_new(params.session_id.clone()) {
        Ok(id) => id,
        Err(_) => unreachable!("test session id must be valid"),
    };
    let config = QueryEngineConfig {
        model_id: "scripted-mock".into(),
        permission_mode: params.permission_mode,
        max_turns: params.max_turns,
        ..Default::default()
    };
    let main_slot = PrebuiltLanguageModelSlot::new(model, coco_inference::RetryConfig::default())
        .with_model_info(coco_query::test_support::default_test_model_info());
    let mut registry_runtimes = vec![(ModelRole::Main, main_slot, Vec::new())];
    if let Some(plan_model) = params.plan_role_model {
        let plan_slot =
            PrebuiltLanguageModelSlot::new(plan_model, coco_inference::RetryConfig::default())
                .with_model_info(coco_query::test_support::default_test_model_info());
        registry_runtimes.push((ModelRole::Plan, plan_slot, Vec::new()));
    }
    let model_runtimes = Arc::new(ModelRuntimeRegistry::from_prebuilt_language_model_roles(
        registry_runtimes,
    ));
    // Wire the agent catalog (Explore/Plan built-ins enabled) — the live
    // source `current_agent_types` reads for `explore_plan_agents_available`
    // and the agent-mention/listing reminders. `session_bootstrap.agents` is
    // `None` in production and is no longer consulted, so the catalog is the
    // only way to make those agents "available" in tests.
    let mut agent_store = coco_subagent::AgentDefinitionStore::new(
        coco_subagent::BuiltinAgentCatalog::all_enabled(),
        coco_subagent::AgentSearchPaths::empty(),
    );
    agent_store.load();
    let engine = QueryEngine::new(
        config,
        session_id,
        model_runtimes,
        params.tools,
        cancel,
        None,
    )
    .with_app_state(params.app_state)
    .with_config_home(params.config_home)
    .with_agent_catalog(agent_store.snapshot())
    // Keep a (default) bootstrap so any `session_bootstrap.is_some()`
    // behavior the harness ran under is preserved; its `agents` field is
    // dead now that `current_agent_types` reads the catalog above.
    .with_session_bootstrap(SessionBootstrap::default())
    // Auto-approve any `Ask` decision (ExitPlanMode, etc.) — tests
    // script the model flow, not user interaction.
    .with_permission_bridge(allow_all_bridge());

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    });

    if params.messages.is_empty() {
        let result = engine
            .run_with_events(&params.prompt_if_empty, tx, coco_types::TurnId::generate())
            .await
            .expect("mock engine run failed");
        let events = collector.await.expect("event collector should join");
        (result, events)
    } else {
        // `run_with_messages` requires at least one `user` message at
        // the tail; callers that want a fresh turn should use
        // `next_turn()` which pushes one before handing off.
        let result = engine
            .run_with_messages(params.messages, tx, coco_types::TurnId::generate())
            .await
            .expect("mock engine run_with_messages failed");
        let events = collector.await.expect("event collector should join");
        (result, events)
    }
}

/// Run the query engine with a mock model and default config.
pub async fn run_with_mock(
    model: Arc<dyn LanguageModel>,
    prompt: &str,
    tools: Arc<ToolRegistry>,
) -> QueryResult {
    let client = coco_query::test_support::model_runtime_registry(model);
    let cancel = CancellationToken::new();
    let config = QueryEngineConfig {
        model_id: "scripted-mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(10),
        ..Default::default()
    };
    let engine = QueryEngine::new(
        config,
        coco_types::SessionId::try_new("test-session").unwrap(),
        client,
        tools,
        cancel,
        None,
    );
    match engine.run(prompt).await {
        Ok(result) => result,
        Err(err) => panic!("mock engine should not fail: {err}"),
    }
}

// ─── Tests using the harness ───

#[tokio::test]
async fn test_harness_text_only() {
    let model = MockModelBuilder::new()
        .then_text("Hello from harness!")
        .build();

    let result = run_with_mock(model, "hi", core_tools()).await;
    assert_eq!(result.response_text, "Hello from harness!");
    assert_eq!(result.turns, 1);
}

#[tokio::test]
async fn test_harness_tool_call_then_text() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "harness test content").unwrap();
    let path = file.to_str().unwrap().to_string();

    let model = MockModelBuilder::new()
        .then_tool_call("Read", serde_json::json!({"file_path": path}))
        .then_text("I read the file.")
        .build();

    let result = run_with_mock(model, "read it", core_tools()).await;
    assert_eq!(result.turns, 2);
    assert_eq!(result.response_text, "I read the file.");
}

#[tokio::test]
async fn test_harness_multi_tool_parallel() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();

    let path_a = dir.path().join("a.txt").to_str().unwrap().to_string();
    let path_b = dir.path().join("b.txt").to_str().unwrap().to_string();

    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::multi_tool(vec![
                ("Read", serde_json::json!({"file_path": path_a.clone()})),
                ("Read", serde_json::json!({"file_path": path_b.clone()})),
            ])
        })
        .then_text("Read both files.")
        .build();

    let result = run_with_mock(model, "read both", core_tools()).await;
    assert_eq!(result.turns, 2);
    assert_eq!(result.response_text, "Read both files.");
}

#[tokio::test]
async fn test_harness_write_edit_read_chain() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("chain.txt");
    let path = file.to_str().unwrap().to_string();

    let p1 = path.clone();
    let p2 = path.clone();
    let p3 = path.clone();

    let model = MockModelBuilder::new()
        // Step 1: Write file
        .on_call(0, move |_| {
            MockResponse::tool_call(
                "Write",
                serde_json::json!({"file_path": p1.clone(), "content": "original content"}),
            )
        })
        // Step 2: Edit file
        .on_call(1, move |_| {
            MockResponse::tool_call(
                "Edit",
                serde_json::json!({
                    "file_path": p2.clone(),
                    "old_string": "original",
                    "new_string": "modified"
                }),
            )
        })
        // Step 3: Read file back
        .on_call(2, move |_| {
            MockResponse::tool_call("Read", serde_json::json!({"file_path": p3.clone()}))
        })
        // Step 4: Final answer
        .then_text("File was written, edited, and verified.")
        .build();

    let result = run_with_mock(model, "write edit read", core_tools()).await;
    assert_eq!(result.turns, 4);
    assert_eq!(
        result.response_text,
        "File was written, edited, and verified."
    );

    // Verify the file has the edited content
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "modified content");
}

#[tokio::test]
async fn test_harness_bash_echo() {
    let model = MockModelBuilder::new()
        .then_tool_call("Bash", serde_json::json!({"command": "echo hello_e2e"}))
        .then_text("Command executed.")
        .build();

    let result = run_with_mock(model, "run echo", core_tools()).await;
    assert_eq!(result.turns, 2);
    assert_eq!(result.response_text, "Command executed.");
}
