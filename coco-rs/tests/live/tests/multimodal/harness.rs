//! Real `QueryEngine` driver for the multimodal suite. No mocked
//! tool layer — `coco_tools::ReadTool` / `coco_tools::BashTool` execute
//! against the OS. The provider is the only thing replaced (with
//! [`CapturingScriptedModel`]) so tests can run without credentials
//! and inspect the engine-assembled prompt the model would have sent
//! to a real provider.
//!
//! Scenarios pre-mint the workdir tempdir via [`fresh_workdir`] so
//! they can build absolute paths into the scripted tool calls (the
//! agent loop runs Read against absolute paths — a relative path
//! would resolve against the worker's process cwd, not the engine
//! cwd_override).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use coco_inference::LanguageModel;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::QueryResult;
use coco_tool_runtime::ToolRegistry;
use coco_types::CoreEvent;
use coco_types::Features;
use coco_types::PermissionMode;
use coco_types::ToolOverrides;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::common;
use crate::multimodal::scripted_model::CapturingScriptedModel;
use crate::multimodal::scripted_model::Reply;

/// Outcome of one scripted run. Owns the workdir tempdir so tests can
/// keep reading files the agent's tools wrote.
pub struct MultimodalOutcome {
    pub result: QueryResult,
    /// Engine-emitted `CoreEvent` stream — kept on the outcome so
    /// follow-on scenarios can assert on tool-use lifecycle without
    /// re-plumbing the channel. Current scenarios pass `result` /
    /// `model.captured_prompts()`.
    #[allow(dead_code)]
    pub events: Vec<CoreEvent>,
    pub model: Arc<CapturingScriptedModel>,
    /// Resolved cwd of the engine. Tests reading the model's writes
    /// (Edit / Write / Bash) consult this — keep alive even though
    /// the current image-read scenarios don't.
    #[allow(dead_code)]
    pub workdir_path: PathBuf,
    pub _workdir: TempDir,
}

/// Pre-mint a tempdir under `/tmp` so scenarios can compute absolute
/// fixture paths before they construct scripted tool calls.
pub fn fresh_workdir() -> Result<TempDir> {
    common::tmpdir::make("coco-tests-multimodal-").with_context(|| "create cwd tempdir under /tmp")
}

/// Drive a real `QueryEngine` against `replies`, register the production
/// `ReadTool` + `BashTool` (so file/image reads happen for real), and
/// return the captured prompts + result. The caller-supplied `workdir`
/// becomes both `project_dir` and `cwd_override` so any tool call that
/// references absolute paths inside it lands on the same disk.
pub async fn run_multimodal_scenario(
    workdir: TempDir,
    replies: Vec<Reply>,
    prompt: &str,
) -> Result<MultimodalOutcome> {
    let workdir_path = workdir.path().to_path_buf();

    let model = CapturingScriptedModel::new(replies);
    let model_runtimes =
        coco_query::test_support::model_runtime_registry(model.clone() as Arc<dyn LanguageModel>);

    let tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(coco_tools::BashTool));
    tool_registry.register(Arc::new(coco_tools::ReadTool));
    let tools = Arc::new(tool_registry);

    let cancel = CancellationToken::new();
    let model_id = model.model_id().to_string();
    let cfg = QueryEngineConfig {
        model_id,
        permission_mode: PermissionMode::BypassPermissions,
        bypass_permissions_available: true,
        context_window: 200_000,
        max_output_tokens: 2_048,
        // Multimodal scenarios are 2-turn: tool_call + final answer.
        // Cap tight so an under-specified `replies` queue can't spin.
        max_turns: Some(4),
        total_token_budget: None,
        system_prompt: Some(
            "You are a deterministic test scripted model. Use tools as instructed.".into(),
        ),
        is_non_interactive: true,
        project_dir: Some(workdir_path.clone()),
        cwd_override: Some(workdir_path.clone()),
        features: Arc::new(Features::with_defaults()),
        tool_overrides: Arc::new(ToolOverrides::none()),
        ..QueryEngineConfig::default()
    };

    let engine = QueryEngine::new(cfg, model_runtimes, tools, cancel, /*hooks*/ None);

    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(512);
    let drainer = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(evt) = event_rx.recv().await {
            events.push(evt);
        }
        events
    });

    let result = engine
        .run_with_events(prompt, event_tx, coco_types::TurnId::generate())
        .await
        .map_err(|e| anyhow::anyhow!("multimodal engine.run_with_events: {e}"))?;
    let events = drainer.await.unwrap_or_default();

    Ok(MultimodalOutcome {
        result,
        events,
        model,
        workdir_path,
        _workdir: workdir,
    })
}
