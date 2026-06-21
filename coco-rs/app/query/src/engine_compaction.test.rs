use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;
use coco_session::storage::ChainWriteOptions;
use coco_types::CacheSafeParams;
use coco_types::ForkLabel;
use coco_types::HookEventType;
use coco_types::HookScope;
use coco_types::ToolAppState;
use pretty_assertions::assert_eq;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::config::QueryEngineConfig;
use crate::engine::QueryEngine;
use crate::forked_agent::ForkDispatcher;
use crate::forked_agent::ForkTranscriptMode;
use crate::forked_agent::ForkedAgentOptions;
use crate::forked_agent::ForkedAgentResult;

#[derive(Default)]
struct CapturingModel {
    options: Mutex<Option<coco_inference::LanguageModelCallOptions>>,
}

#[async_trait::async_trait]
impl coco_inference::LanguageModel for CapturingModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "mock-model"
    }

    async fn do_generate(
        &self,
        options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelGenerateResult, coco_inference::AISdkError> {
        *self.options.lock().expect("model options lock poisoned") = Some(options.clone());
        Ok(coco_inference::LanguageModelGenerateResult {
            content: vec![coco_llm_types::AssistantContentPart::Text(
                coco_llm_types::TextPart {
                    text: "direct summary".into(),
                    provider_metadata: None,
                },
            )],
            usage: coco_llm_types::Usage::new(0, 0),
            finish_reason: coco_llm_types::FinishReason::new(coco_llm_types::StopReason::EndTurn),
            warnings: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelStreamResult, coco_inference::AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

struct EmptySummaryModel;

#[async_trait::async_trait]
impl coco_inference::LanguageModel for EmptySummaryModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "mock-model"
    }

    async fn do_generate(
        &self,
        _options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelGenerateResult, coco_inference::AISdkError> {
        Ok(coco_inference::LanguageModelGenerateResult {
            content: Vec::new(),
            usage: coco_llm_types::Usage::new(0, 0),
            finish_reason: coco_llm_types::FinishReason::new(coco_llm_types::StopReason::EndTurn),
            warnings: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        options: &coco_inference::LanguageModelCallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelStreamResult, coco_inference::AISdkError> {
        let result = self.do_generate(options, abort_signal).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

#[derive(Default)]
struct CapturingCompactDispatcher {
    cache: Mutex<Option<CacheSafeParams>>,
    options: Mutex<Option<ForkedAgentOptions>>,
    prompt: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl ForkDispatcher for CapturingCompactDispatcher {
    async fn dispatch(
        &self,
        cache: &CacheSafeParams,
        options: &ForkedAgentOptions,
        prompt: &str,
        _system_prompt_override: Option<String>,
    ) -> Result<ForkedAgentResult, coco_error::BoxedError> {
        *self.cache.lock().expect("cache lock poisoned") = Some(cache.clone());
        *self.options.lock().expect("options lock poisoned") = Some(options.clone());
        *self.prompt.lock().expect("prompt lock poisoned") = Some(prompt.to_string());
        Ok(ForkedAgentResult {
            messages: vec![Arc::new(assistant_msg("fork summary"))],
            ..Default::default()
        })
    }
}

struct FailingDispatcher;

#[async_trait::async_trait]
impl ForkDispatcher for FailingDispatcher {
    async fn dispatch(
        &self,
        _cache: &CacheSafeParams,
        _options: &ForkedAgentOptions,
        _prompt: &str,
        _system_prompt_override: Option<String>,
    ) -> Result<ForkedAgentResult, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "fork failed",
            coco_error::StatusCode::Internal,
        )))
    }
}

#[derive(Default)]
struct CapturingSessionStartSink {
    effects: Mutex<Vec<crate::session_start_hooks::SessionStartHookSideEffects>>,
}

#[async_trait::async_trait]
impl crate::session_start_hooks::SessionStartHookSideEffectSink for CapturingSessionStartSink {
    async fn handle_session_start_hook_side_effects(
        &self,
        effects: crate::session_start_hooks::SessionStartHookSideEffects,
    ) {
        self.effects
            .lock()
            .expect("side-effect sink lock poisoned")
            .push(effects);
    }
}

fn new_engine(
    model: Arc<CapturingModel>,
    dispatcher: Option<Arc<dyn ForkDispatcher>>,
) -> QueryEngine {
    let model_runtimes = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(coco_tool_runtime::ToolRegistry::new());
    let mut engine = QueryEngine::new(
        QueryEngineConfig::default(),
        model_runtimes,
        tools,
        CancellationToken::new(),
        None,
    );
    if let Some(dispatcher) = dispatcher {
        engine = engine.with_fork_dispatcher(dispatcher);
    }
    engine
}

fn new_engine_for_model(model: Arc<dyn coco_inference::LanguageModel>) -> QueryEngine {
    let model_runtimes = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(coco_tool_runtime::ToolRegistry::new());
    QueryEngine::new(
        QueryEngineConfig::default(),
        model_runtimes,
        tools,
        CancellationToken::new(),
        None,
    )
}

fn new_engine_with_tools(
    model: Arc<dyn coco_inference::LanguageModel>,
    tools: Arc<coco_tool_runtime::ToolRegistry>,
) -> QueryEngine {
    let model_info = coco_config::ModelInfo::from_partial(
        "mock",
        "mock-model",
        coco_config::PartialModelInfo {
            context_window: Some(coco_config::PositiveTokens::new(200_000)),
            max_output_tokens: Some(coco_config::PositiveTokens::new(8_192)),
            capabilities: Some(vec![coco_types::Capability::ClientSideToolSearch]),
            ..Default::default()
        },
    )
    .expect("valid test model info");
    let model_runtimes = Arc::new(
        coco_inference::ModelRuntimeRegistry::from_prebuilt_language_model(
            coco_types::ModelRole::Main,
            coco_inference::PrebuiltLanguageModelSlot::new(
                model,
                coco_inference::RetryConfig::default(),
            )
            .with_model_info(model_info),
        ),
    );
    QueryEngine::new(
        QueryEngineConfig::default(),
        model_runtimes,
        tools,
        CancellationToken::new(),
        None,
    )
}

fn new_engine_with_hooks(
    model: Arc<CapturingModel>,
    hooks: Arc<coco_hooks::HookRegistry>,
    sync_hook_buffer: coco_hooks::SyncHookEventBuffer,
    side_effect_sink: Option<crate::session_start_hooks::SessionStartHookSideEffectSinkRef>,
) -> QueryEngine {
    let model_runtimes = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(coco_tool_runtime::ToolRegistry::new());
    let mut engine = QueryEngine::new(
        QueryEngineConfig::default(),
        model_runtimes,
        tools,
        CancellationToken::new(),
        Some(hooks),
    )
    .with_sync_hook_buffer(sync_hook_buffer);
    if let Some(sink) = side_effect_sink {
        engine = engine.with_session_start_hook_side_effect_sink(sink);
    }
    engine
}

fn hook(event: HookEventType, command: &str) -> coco_hooks::HookDefinition {
    coco_hooks::HookDefinition {
        event,
        matcher: None,
        handler: coco_hooks::HookHandler::Command {
            command: command.to_string(),
            timeout_ms: Some(5000),
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    }
}

#[tokio::test]
async fn plan_mode_snapshot_omits_always_loaded_exit_plan_mode() {
    let registry = coco_tool_runtime::ToolRegistry::new();
    registry.register(Arc::new(coco_tools::ExitPlanModeTool));
    let app_state = Arc::new(RwLock::new(ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(coco_types::PermissionMode::Plan),
            ..Default::default()
        },
        ..ToolAppState::default()
    }));
    let engine = new_engine_with_tools(Arc::new(CapturingModel::default()), Arc::new(registry))
        .with_app_state(app_state);

    let attachment = engine
        .snapshot_plan_mode_attachment()
        .await
        .expect("plan mode should snapshot");

    // ExitPlanMode is non-deferred (always loaded), so it never lands in the
    // snapshot's deferred-tools list — the post-compact "reload via ToolSearch"
    // path no longer applies to it (it survives compaction already loaded).
    assert!(
        attachment.deferred_tools.is_empty(),
        "always-loaded ExitPlanMode must not be listed as deferred: {:?}",
        attachment.deferred_tools
    );
}

fn compact_attempt(summary_request: &str) -> coco_compact::CompactSummaryAttempt {
    coco_compact::CompactSummaryAttempt {
        messages: vec![std::sync::Arc::new(coco_messages::create_user_message(
            "conversation slice only",
        ))],
        context_messages: vec![std::sync::Arc::new(coco_messages::create_user_message(
            "conversation context for api",
        ))],
        summary_request: summary_request.to_string(),
        prompt_kind: coco_compact::CompactSummaryKind::Full,
        pre_compact_tokens: 42,
        max_summary_tokens: 20_000,
    }
}

fn assistant_msg(text: &str) -> coco_messages::Message {
    coco_messages::Message::Assistant(coco_messages::AssistantMessage {
        message: coco_messages::LlmMessage::Assistant {
            content: vec![coco_messages::AssistantContent::Text(
                coco_messages::TextContent {
                    text: text.into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        uuid: uuid::Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: Some(coco_messages::StopReason::EndTurn),
        usage: Some(coco_types::TokenUsage::default()),
        cost_usd: None,
        request_id: Some("req-compact".into()),
        api_error: None,
    })
}

fn assistant_msg_with_usage(
    text: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> coco_messages::Message {
    let mut msg = assistant_msg(text);
    if let coco_messages::Message::Assistant(assistant) = &mut msg {
        assistant.usage = Some(coco_types::TokenUsage {
            input_tokens: coco_types::InputTokens {
                total: input_tokens,
                ..Default::default()
            },
            output_tokens: coco_types::OutputTokens {
                total: output_tokens,
                ..Default::default()
            },
        });
    }
    msg
}

fn compactable_history() -> coco_messages::MessageHistory {
    let mut history = coco_messages::MessageHistory::new();
    for idx in 0..4 {
        history.push(coco_messages::create_user_message(&format!("user {idx}")));
        history.push(assistant_msg(&format!("assistant {idx}")));
    }
    history
}

fn user_text(message: &coco_messages::Message) -> Option<&str> {
    let coco_messages::Message::User(user) = message else {
        return None;
    };
    let LlmMessage::User { content, .. } = &user.message else {
        return None;
    };
    content.iter().find_map(|part| match part {
        UserContentPart::Text(text) => Some(text.text.as_str()),
        _ => None,
    })
}

fn drain_protocol_events(
    rx: &mut tokio::sync::mpsc::Receiver<coco_types::CoreEvent>,
) -> Vec<coco_types::ServerNotification> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let coco_types::CoreEvent::Protocol(notification) = event {
            events.push(notification);
        }
    }
    events
}

fn chain_options(cwd: &str) -> ChainWriteOptions {
    ChainWriteOptions {
        cwd: cwd.to_string(),
        timestamp: "2026-01-02T03:04:05Z".to_string(),
        is_sidechain: false,
        agent_id: None,
        starting_parent_uuid: None,
        git_branch: None,
    }
}

fn empty_cache() -> CacheSafeParams {
    CacheSafeParams {
        rendered_system_prompt: "system".into(),
        model_id: "mock-model".into(),
        provider: "mock".into(),
        active_shell_tool: coco_types::ActiveShellTool::Bash,
        prompt_cache: None,
        fork_context_messages: vec![Arc::new(coco_messages::create_user_message(
            "old parent cache",
        ))],
    }
}

#[tokio::test]
async fn manual_compact_empty_history_emits_notice_without_failure() {
    let model = Arc::new(CapturingModel::default());
    let engine = new_engine(model, None);
    let mut history = coco_messages::MessageHistory::new();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let event_tx = Some(tx);

    let outcome = engine
        .run_manual_compact(
            &mut history,
            &event_tx,
            crate::ManualCompactRequest::new(None),
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Skipped);
    let events = drain_protocol_events(&mut rx);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, coco_types::ServerNotification::CompactionFailed(_)))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        coco_types::ServerNotification::CompactionPhase(p)
            if p.phase == coco_types::CompactionPhase::Done
    )));
    let rendered = format!("{:?}", history.as_slice());
    assert!(rendered.contains("<command-name>/compact</command-name>"));
    assert!(
        rendered.contains("<local-command-stdout>No messages to compact.</local-command-stdout>")
    );
}

#[tokio::test]
async fn manual_compact_single_round_enters_llm_compact_when_summary_is_returned() {
    let model = Arc::new(CapturingModel::default());
    let engine = new_engine(model.clone(), None);
    let mut history = coco_messages::MessageHistory::new();
    history.push(coco_messages::create_user_message("one round"));
    history.push(assistant_msg("assistant"));
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let event_tx = Some(tx);

    let outcome = engine
        .run_manual_compact(
            &mut history,
            &event_tx,
            crate::ManualCompactRequest::new(Some("focus".to_string())),
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Applied);
    assert!(
        model
            .options
            .lock()
            .expect("model options lock poisoned")
            .is_some(),
        "single-round manual compact should reach the LLM summarizer"
    );
    let events = drain_protocol_events(&mut rx);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, coco_types::ServerNotification::CompactionFailed(_)))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        coco_types::ServerNotification::CompactionPhase(p)
            if p.phase == coco_types::CompactionPhase::Done
    )));
    let rendered = format!("{:?}", history.as_slice());
    assert!(rendered.contains("<command-name>/compact</command-name>"));
    assert!(rendered.contains("direct summary"));
    assert!(rendered.contains("<local-command-stdout>Compacted ("));
}

#[tokio::test]
async fn manual_compact_summarizer_error_emits_compaction_failed() {
    let engine = new_engine_for_model(Arc::new(EmptySummaryModel));
    let mut history = compactable_history();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let event_tx = Some(tx);

    let outcome = engine
        .run_manual_compact(
            &mut history,
            &event_tx,
            crate::ManualCompactRequest::new(Some("focus".to_string())),
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Failed);
    let events = drain_protocol_events(&mut rx);
    assert!(events.iter().any(|event| matches!(
        event,
        coco_types::ServerNotification::CompactionFailed(p)
            if p.error.starts_with("Error during compaction:")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        coco_types::ServerNotification::CompactionPhase(p)
            if p.phase == coco_types::CompactionPhase::Done
    )));
}

#[tokio::test]
async fn compact_preserved_segment_round_trips_through_transcript_store_resume() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = "/compact-preserved-test";
    let paths = Arc::new(coco_paths::ProjectPaths::new(
        dir.path().to_path_buf(),
        std::path::Path::new(cwd),
    ));
    let store = coco_session::TranscriptStore::new(paths);
    let session_id = "session-compact-preserved";
    let mut seen = HashSet::new();

    let old_user = Arc::new(coco_messages::create_user_message("old prefix user"));
    let old_assistant = Arc::new(assistant_msg("old prefix assistant"));
    let head = Arc::new(coco_messages::create_user_message("preserved user"));
    let tail = Arc::new(assistant_msg_with_usage(
        "preserved assistant",
        /*input_tokens*/ 190_000,
        /*output_tokens*/ 200,
    ));
    let pre_compact = vec![
        old_user.clone(),
        old_assistant.clone(),
        head.clone(),
        tail.clone(),
    ];
    store
        .append_message_chain(
            session_id,
            pre_compact.iter().map(Arc::as_ref),
            &mut seen,
            chain_options(cwd),
        )
        .expect("pre-compact history writes");

    let result = coco_compact::compact_conversation(
        &pre_compact,
        &coco_compact::CompactRunOptions {
            keep_recent_rounds: 1,
            trigger: coco_types::CompactTrigger::Manual,
            ..Default::default()
        },
        |_attempt| async {
            Ok(coco_compact::CompactSummaryResponse {
                summary: "compact summary".to_string(),
            })
        },
        None,
    )
    .await
    .expect("compact succeeds");

    let boundary_uuid = *result
        .boundary_marker
        .uuid()
        .expect("boundary uuid should exist");
    let summary_uuid = *result.summary_messages[0]
        .uuid()
        .expect("summary uuid should exist");
    let tail_uuid = *tail.uuid().expect("tail uuid should exist");
    let kept_uuids = result
        .messages_to_keep
        .iter()
        .filter_map(|message| message.uuid().copied())
        .collect::<Vec<_>>();
    assert!(
        kept_uuids.contains(&tail_uuid),
        "preserved segment should include the final assistant"
    );
    let summarized_uuids = pre_compact
        .iter()
        .filter_map(|message| message.uuid().copied())
        .filter(|uuid| !kept_uuids.contains(uuid))
        .collect::<Vec<_>>();
    match &result.boundary_marker {
        coco_messages::Message::System(coco_messages::SystemMessage::CompactBoundary(boundary)) => {
            let segment = boundary
                .preserved_segment
                .as_ref()
                .expect("compact should annotate preserved segment");
            assert_eq!(segment.head_uuid, kept_uuids[0]);
            assert_eq!(segment.tail_uuid, *kept_uuids.last().expect("kept uuid"));
            assert_eq!(segment.anchor_uuid, boundary_uuid);
        }
        other => panic!("expected compact boundary, got {other:?}"),
    }

    let post_compact = coco_compact::build_post_compact_messages(&result);
    store
        .append_message_chain(
            session_id,
            post_compact.iter().map(Arc::as_ref),
            &mut seen,
            chain_options(cwd),
        )
        .expect("post-compact history writes");

    let state =
        coco_session::recovery::load_session_state_for_resume(&store.transcript_path(session_id))
            .expect("resume state loads");

    for uuid in [boundary_uuid, summary_uuid]
        .into_iter()
        .chain(kept_uuids.iter().copied())
    {
        assert!(
            state.selected_chain_uuids.contains(&uuid.to_string()),
            "expected selected resume chain to contain {uuid}"
        );
    }
    for uuid in summarized_uuids {
        assert!(
            !state.selected_chain_uuids.contains(&uuid.to_string()),
            "old compacted prefix should be pruned from selected resume chain"
        );
    }
    let resumed_uuids = state
        .messages
        .iter()
        .filter_map(coco_messages::Message::uuid)
        .copied()
        .collect::<Vec<_>>();
    let mut expected_uuids = vec![boundary_uuid];
    expected_uuids.extend(kept_uuids);
    expected_uuids.push(summary_uuid);
    assert_eq!(
        resumed_uuids, expected_uuids,
        "resume should relink preserved messages under the compact boundary"
    );
    assert_eq!(
        state.total_input_tokens, 0,
        "preserved assistant input usage must be zeroed on resume"
    );
    assert_eq!(
        state.total_output_tokens, 0,
        "preserved assistant output usage must be zeroed on resume"
    );
}

#[tokio::test]
async fn manual_compact_success_appends_slash_breadcrumbs_before_hook_results() {
    let model = Arc::new(CapturingModel::default());
    let hooks = Arc::new(coco_hooks::HookRegistry::new());
    hooks.register(hook(
        HookEventType::SessionStart,
        "echo session-hook-output",
    ));
    let engine = new_engine_with_hooks(model, hooks, coco_hooks::SyncHookEventBuffer::new(), None);
    let mut history = compactable_history();

    let outcome = engine
        .run_manual_compact(
            &mut history,
            &None,
            crate::ManualCompactRequest::new(Some("keep build errors".to_string())),
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Applied);
    let rendered = format!("{:?}", history.as_slice());
    assert!(rendered.contains("<local-command-caveat>Caveat:"));
    assert!(rendered.contains("<command-name>/compact</command-name>"));
    assert!(rendered.contains("<command-args>keep build errors</command-args>"));
    assert!(rendered.contains("<local-command-stdout>Compacted ("));
    assert!(rendered.contains("tokens, saved"));
    assert!(rendered.contains("Ctrl+O to see full summary)</local-command-stdout>"));
    let compact_breadcrumbs = history
        .as_slice()
        .iter()
        .filter_map(|msg| {
            let coco_messages::Message::User(user) = msg.as_ref() else {
                return None;
            };
            let text = user_text(msg.as_ref())?;
            (text.contains("<command-name>/compact</command-name>")
                || text.contains("<local-command-stdout>Compacted"))
            .then_some(user)
        })
        .collect::<Vec<_>>();
    assert_eq!(compact_breadcrumbs.len(), 2);
    for user in compact_breadcrumbs {
        assert_eq!(
            user.origin,
            Some(coco_messages::MessageOrigin::SlashCommand)
        );
        assert!(
            !user.is_visible_in_transcript_only,
            "manual compact breadcrumbs must remain model-visible"
        );
    }
    let stdout_idx = rendered
        .find("<local-command-stdout>")
        .expect("stdout breadcrumb should be present");
    let hook_idx = rendered
        .find("session-hook-output")
        .expect("hook result should be present");
    assert!(
        stdout_idx < hook_idx,
        "slash breadcrumbs must precede post-compact hook results"
    );
}

#[tokio::test]
async fn manual_compact_with_args_passes_instructions_to_summarizer() {
    let model = Arc::new(CapturingModel::default());
    let engine = new_engine(model.clone(), None);
    let mut history = compactable_history();

    let outcome = engine
        .run_manual_compact(
            &mut history,
            &None,
            crate::ManualCompactRequest::new(Some("focus on auth regressions".to_string())),
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Applied);
    let options = model
        .options
        .lock()
        .expect("model options lock poisoned")
        .clone()
        .expect("direct model should be called");
    let rendered_prompt = format!("{:?}", options.prompt);
    assert!(rendered_prompt.contains("Additional Instructions"));
    assert!(rendered_prompt.contains("focus on auth regressions"));
}

#[tokio::test]
async fn compact_summary_uses_cache_safe_fork_with_deny_all_tools() {
    let model = Arc::new(CapturingModel::default());
    let dispatcher = Arc::new(CapturingCompactDispatcher::default());
    let engine = new_engine(model, Some(dispatcher.clone()));
    engine.save_cache_safe_params(empty_cache()).await;

    let response = engine
        .run_compact_summary_attempt(compact_attempt("summarize now"))
        .await
        .expect("fork summary should succeed");

    assert_eq!(response.summary, "fork summary");
    assert_eq!(
        dispatcher
            .prompt
            .lock()
            .expect("prompt lock poisoned")
            .as_deref(),
        Some("summarize now")
    );

    let options = dispatcher
        .options
        .lock()
        .expect("options lock poisoned")
        .clone()
        .expect("dispatcher should capture options");
    assert_eq!(options.fork_label, ForkLabel::Compact);
    assert_eq!(options.max_turns, Some(1));
    assert_eq!(options.transcript_mode, ForkTranscriptMode::Sidechain);
    assert!(options.skip_cache_write);
    assert!(options.can_use_tool.is_some());
    assert!(options.require_can_use_tool);

    let cache = dispatcher
        .cache
        .lock()
        .expect("cache lock poisoned")
        .clone()
        .expect("dispatcher should capture cache");
    let serialized =
        serde_json::to_string(&cache.fork_context_messages).expect("fork context should serialize");
    assert!(serialized.contains("conversation context for api"));
    assert!(!serialized.contains("conversation slice only"));
    assert!(!serialized.contains("old parent cache"));
}

#[tokio::test]
async fn compact_summary_falls_back_to_direct_no_tools_call() {
    let model = Arc::new(CapturingModel::default());
    let engine = new_engine(model.clone(), Some(Arc::new(FailingDispatcher)));
    engine.save_cache_safe_params(empty_cache()).await;

    let mut attempt = compact_attempt("direct request");
    attempt.max_summary_tokens = 123;
    let response = engine
        .run_compact_summary_attempt(attempt)
        .await
        .expect("direct fallback should succeed");

    assert_eq!(response.summary, "direct summary");
    let options = model
        .options
        .lock()
        .expect("model options lock poisoned")
        .clone()
        .expect("direct model should be called");
    assert!(
        options.tools.is_none(),
        "direct compact must not expose tools"
    );
    assert!(options.tool_choice.is_none());
    assert_eq!(options.max_output_tokens, Some(123));
    assert_eq!(options.prompt.len(), 2);
    assert!(format!("{:?}", options.prompt[0]).contains("conversation context for api"));
    assert!(!format!("{:?}", options.prompt[0]).contains("conversation slice only"));
    match options.prompt.last() {
        Some(LlmMessage::User { content, .. }) => {
            assert!(content.iter().any(|part| matches!(
                part,
                UserContentPart::Text(text) if text.text == "direct request"
            )));
        }
        other => panic!("summary request should be appended as a user message, got {other:?}"),
    }
}

#[tokio::test]
async fn partial_compact_runs_hooks_and_inlines_session_start_results() {
    let model = Arc::new(CapturingModel::default());
    let hooks = Arc::new(coco_hooks::HookRegistry::new());
    hooks.register(hook(HookEventType::PreCompact, "echo pre-hook-instruction"));
    hooks.register(hook(HookEventType::PostCompact, "echo post-hook-output"));
    hooks.register(hook(
        HookEventType::SessionStart,
        "echo session-hook-output",
    ));
    let sync = coco_hooks::SyncHookEventBuffer::new();
    let engine = new_engine_with_hooks(model.clone(), hooks, sync.clone(), None);

    let mut history = coco_messages::MessageHistory::new();
    history.push(coco_messages::create_user_message("kept prefix"));
    history.push(assistant_msg("kept assistant"));
    history.push(coco_messages::create_user_message("summarize tail"));
    history.push(assistant_msg("tail assistant"));

    let outcome = engine
        .run_partial_compact(
            &mut history,
            &None,
            2,
            coco_messages::PartialCompactDirection::Newest,
            Some("focus user feedback".to_string()),
            Some("user compact instruction".to_string()),
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Applied);
    let options = model
        .options
        .lock()
        .expect("model options lock poisoned")
        .clone()
        .expect("direct model should be called");
    let rendered_prompt = format!("{:?}", options.prompt);
    assert!(rendered_prompt.contains("user compact instruction"));
    assert!(rendered_prompt.contains("pre-hook-instruction"));
    assert!(rendered_prompt.contains("focus user feedback"));

    let rendered_history = format!("{:?}", history.as_slice());
    assert!(rendered_history.contains("session-hook-output"));
    assert!(
        sync.drain().await.is_empty(),
        "partial compact should not duplicate SessionStart output into next-turn hook buffer"
    );
}

#[tokio::test]
async fn partial_compact_applies_session_start_aggregate_side_effects() {
    let model = Arc::new(CapturingModel::default());
    let hooks = Arc::new(coco_hooks::HookRegistry::new());
    hooks.register(hook(
        HookEventType::SessionStart,
        "printf '{\"initialUserMessage\":\"hook initial turn\",\"watchPaths\":[\"/tmp/coco-watch\"]}'",
    ));
    let sync = coco_hooks::SyncHookEventBuffer::new();
    let sink = Arc::new(CapturingSessionStartSink::default());
    let engine = new_engine_with_hooks(model, hooks, sync.clone(), Some(sink.clone()));

    let mut history = coco_messages::MessageHistory::new();
    history.push(coco_messages::create_user_message("kept prefix"));
    history.push(assistant_msg("kept assistant"));
    history.push(coco_messages::create_user_message("summarize tail"));
    history.push(assistant_msg("tail assistant"));

    let outcome = engine
        .run_partial_compact(
            &mut history,
            &None,
            2,
            coco_messages::PartialCompactDirection::Newest,
            None,
            None,
        )
        .await;

    assert_eq!(outcome, coco_compact::CompactOutcome::Applied);
    let rendered_history = format!("{:?}", history.as_slice());
    assert!(rendered_history.contains("hook initial turn"));
    let effects = sink
        .effects
        .lock()
        .expect("side-effect sink lock poisoned")
        .clone();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0].initial_user_message.as_deref(),
        Some("hook initial turn")
    );
    assert_eq!(effects[0].watch_paths, vec!["/tmp/coco-watch"]);
    assert!(
        sync.drain().await.is_empty(),
        "partial compact should not duplicate SessionStart output into next-turn hook buffer"
    );
}
