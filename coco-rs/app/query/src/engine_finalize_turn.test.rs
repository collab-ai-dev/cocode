// Tests for `render_teammate_message_wrapper` lived here. The helper
// was deleted alongside the engine-side `Inbox`: teammate messages now
// flow through `CommandQueue` with `QueueOrigin::Coordinator` /
// `QueueOrigin::TaskNotification`, and the drain at
// `helpers::queued_command_to_attachment` applies origin-specific
// framing via `wrap_command_text`. Coordinator messages surface as
// `queued_command` attachments, not as a separate
// `<teammate-message>` envelope.

// Phase 7 — Wire stub-field tests for `build_suggestion_context`.
//
// These assert the three previously-stubbed `SuggestionContext`
// fields (`pending_permission`, `elicitation_active`, `rate_limit`)
// now reflect live state on `ToolAppState`. Each test seeds the
// relevant counter / map, calls `build_suggestion_context`, and
// asserts the field flips on/off.

use super::build_suggestion_context;
use crate::CoreEvent;
use crate::ServerNotification;
use crate::config::QueryEngineConfig;
use crate::engine::QueryEngine;
use crate::forked_agent::ForkDispatcher;
use crate::forked_agent::ForkedAgentOptions;
use crate::forked_agent::ForkedAgentResult;
use coco_types::CacheSafeParams;
use coco_types::PendingPermissionGuard;
use coco_types::ProviderApi;
use coco_types::RateLimitEntry;
use coco_types::RateLimitStatus;
use coco_types::TokenUsage;
use coco_types::ToolAppState;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn empty_cache(provider: &str) -> CacheSafeParams {
    CacheSafeParams {
        rendered_system_prompt: String::new(),
        model_id: "claude-opus-4-7".into(),
        provider: provider.into(),
        active_shell_tool: coco_types::ActiveShellTool::Bash,
        prompt_cache: None,
        fork_context_messages: Vec::new(),
    }
}

fn assistant_msg(text: &str, request_id: Option<&str>) -> coco_messages::Message {
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
        usage: Some(TokenUsage::default()),
        cost_usd: None,
        request_id: request_id.map(str::to_string),
        api_error: None,
    })
}

struct DummyModel;

#[async_trait::async_trait]
impl coco_inference::LanguageModel for DummyModel {
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
            content: vec![coco_llm_types::AssistantContentPart::Text(
                coco_llm_types::TextPart {
                    text: "unused".into(),
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

#[derive(Default)]
struct CapturingSuggestionDispatcher {
    prompt: std::sync::Mutex<Option<String>>,
    system_override: std::sync::Mutex<Option<Option<String>>>,
}

#[async_trait::async_trait]
impl ForkDispatcher for CapturingSuggestionDispatcher {
    async fn dispatch(
        &self,
        _cache: &CacheSafeParams,
        _options: &ForkedAgentOptions,
        prompt: &str,
        system_prompt_override: Option<String>,
    ) -> Result<ForkedAgentResult, coco_error::BoxedError> {
        *self.prompt.lock().expect("prompt lock is not poisoned") = Some(prompt.to_string());
        *self
            .system_override
            .lock()
            .expect("system override lock is not poisoned") = Some(system_prompt_override);
        Ok(ForkedAgentResult {
            messages: vec![Arc::new(assistant_msg(
                "run cargo check",
                Some("req-suggest"),
            ))],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn build_suggestion_context_pending_permission_reflects_counter() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let cache = empty_cache("anthropic");

    // Counter at 0 → field is false.
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(!ctx.pending_permission, "counter == 0 should give false");

    // Acquire a guard → counter == 1 → field flips true.
    let counter = app_state.read().await.pending_permission_count.clone();
    let guard = PendingPermissionGuard::acquire(counter);
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(ctx.pending_permission, "counter > 0 should give true");

    // Drop guard → counter back to 0 → field flips false again.
    drop(guard);
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(
        !ctx.pending_permission,
        "guard drop should decrement counter"
    );
}

#[tokio::test]
async fn build_suggestion_context_elicitation_active_reflects_counter() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let cache = empty_cache("anthropic");

    let counter = app_state.read().await.elicitation_pending_count.clone();
    counter.fetch_add(1, Ordering::Relaxed);
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(ctx.elicitation_active);

    counter.fetch_sub(1, Ordering::Relaxed);
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(!ctx.elicitation_active);
}

#[tokio::test]
async fn build_suggestion_context_rate_limit_selective_by_provider() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    // Insert a Rejected entry for Anthropic with a future reset.
    {
        let mut snap = app_state.write().await;
        let now = chrono::Utc::now().timestamp_millis();
        snap.rate_limits.insert(
            "anthropic".to_string(),
            RateLimitEntry {
                api: ProviderApi::Anthropic,
                status: RateLimitStatus::Rejected,
                reset_at_ms: Some(now + 60_000),
                retry_after_seconds: Some(60),
                last_observed_ms: now,
            },
        );
    }

    // Cache provider "anthropic" → suppress.
    let cache = empty_cache("anthropic");
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(
        ctx.rate_limit,
        "Rejected entry on cache.provider should suppress"
    );

    // Cache provider "openai" (different) → no suppression
    // (selectivity).
    let cache = empty_cache("openai");
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(
        !ctx.rate_limit,
        "Rejected entry on a different provider must not suppress (selective policy)"
    );
}

#[tokio::test]
async fn build_suggestion_context_rate_limit_expires_with_reset_at() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    // Insert a Rejected entry with a reset time already in the past.
    {
        let mut snap = app_state.write().await;
        let now = chrono::Utc::now().timestamp_millis();
        snap.rate_limits.insert(
            "anthropic".to_string(),
            RateLimitEntry {
                api: ProviderApi::Anthropic,
                status: RateLimitStatus::Rejected,
                reset_at_ms: Some(now - 60_000), // already expired
                retry_after_seconds: Some(60),
                last_observed_ms: now - 120_000,
            },
        );
    }

    let cache = empty_cache("anthropic");
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(
        !ctx.rate_limit,
        "expired Rejected entry must not suppress (defensive read-side check)"
    );
}

#[tokio::test]
async fn build_suggestion_context_rate_limit_empty_provider_fails_open() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    {
        let mut snap = app_state.write().await;
        let now = chrono::Utc::now().timestamp_millis();
        snap.rate_limits.insert(
            "anthropic".to_string(),
            RateLimitEntry {
                api: ProviderApi::Anthropic,
                status: RateLimitStatus::Rejected,
                reset_at_ms: Some(now + 60_000),
                retry_after_seconds: Some(60),
                last_observed_ms: now,
            },
        );
    }

    // Pre-Phase-7 transcripts deserialize with `provider: ""`. We
    // can't match selectively without a key, so we fail open
    // (no suppression) rather than silencing all suggestions.
    let cache = empty_cache("");
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(
        !ctx.rate_limit,
        "empty cache.provider must fail open even when entries exist"
    );
}

#[tokio::test]
async fn maybe_spawn_prompt_suggestion_emits_protocol_event() {
    let model = Arc::new(DummyModel);
    let model_runtimes = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(coco_tool_runtime::ToolRegistry::new());
    let dispatcher = Arc::new(CapturingSuggestionDispatcher::default());
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let engine = QueryEngine::new(
        QueryEngineConfig::default(),
        model_runtimes,
        tools,
        CancellationToken::new(),
        None,
    )
    .with_app_state(app_state.clone())
    .with_fork_dispatcher(dispatcher.clone());

    let mut cache = empty_cache("anthropic");
    cache.fork_context_messages = vec![
        Arc::new(assistant_msg("first turn", Some("req-parent-1"))),
        Arc::new(assistant_msg("second turn", Some("req-parent-2"))),
    ];
    engine.save_cache_safe_params(cache).await;

    let (tx, mut rx) = mpsc::channel(4);
    engine
        .maybe_spawn_prompt_suggestion_after_stop(&Some(tx))
        .await;

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("prompt suggestion event should arrive")
        .expect("event channel should stay open until event");
    match event {
        CoreEvent::Protocol(ServerNotification::PromptSuggestion { suggestions }) => {
            assert_eq!(suggestions, vec!["run cargo check".to_string()]);
        }
        other => panic!("expected PromptSuggestion protocol event, got {other:?}"),
    }

    let prompt = dispatcher
        .prompt
        .lock()
        .expect("prompt lock is not poisoned")
        .clone()
        .expect("dispatcher should receive prompt");
    assert_eq!(prompt, crate::prompt_suggestion::SUGGESTION_PROMPT);
    let override_seen = dispatcher
        .system_override
        .lock()
        .expect("system override lock is not poisoned")
        .clone()
        .expect("dispatcher should record override argument");
    assert!(
        override_seen.is_none(),
        "promptSuggestion must use user prompt only, no system override"
    );
}

#[tokio::test]
async fn prune_stale_rate_limits_removes_expired_entries() {
    use super::prune_stale_rate_limits;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let now = chrono::Utc::now().timestamp_millis();

    {
        let mut snap = app_state.write().await;
        // Expired (reset 60s ago).
        snap.rate_limits.insert(
            "anthropic".to_string(),
            RateLimitEntry {
                api: ProviderApi::Anthropic,
                status: RateLimitStatus::Rejected,
                reset_at_ms: Some(now - 60_000),
                retry_after_seconds: None,
                last_observed_ms: now - 120_000,
            },
        );
        // Still active (reset 60s in future).
        snap.rate_limits.insert(
            "openai".to_string(),
            RateLimitEntry {
                api: ProviderApi::Openai,
                status: RateLimitStatus::Rejected,
                reset_at_ms: Some(now + 60_000),
                retry_after_seconds: None,
                last_observed_ms: now,
            },
        );
        // None reset → retained until overwritten.
        snap.rate_limits.insert(
            "google".to_string(),
            RateLimitEntry {
                api: ProviderApi::Gemini,
                status: RateLimitStatus::Rejected,
                reset_at_ms: None,
                retry_after_seconds: None,
                last_observed_ms: now,
            },
        );
    }

    prune_stale_rate_limits(&app_state).await;

    let snap = app_state.read().await;
    assert!(
        !snap.rate_limits.contains_key("anthropic"),
        "expired anthropic entry should be pruned"
    );
    assert!(
        snap.rate_limits.contains_key("openai"),
        "still-active openai entry should be retained"
    );
    assert!(
        snap.rate_limits.contains_key("google"),
        "None-reset entry should be retained until overwritten"
    );
}

#[tokio::test]
async fn record_rate_limit_observation_writes_entry() {
    use crate::engine_helpers::record_rate_limit_observation;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    record_rate_limit_observation(
        &app_state,
        "anthropic",
        ProviderApi::Anthropic,
        Some(45_000), // 45s retry-after
    )
    .await;

    let snap = app_state.read().await;
    let entry = snap
        .rate_limits
        .get("anthropic")
        .expect("entry should be inserted");
    assert_eq!(entry.api, ProviderApi::Anthropic);
    assert_eq!(entry.status, RateLimitStatus::Rejected);
    assert_eq!(entry.retry_after_seconds, Some(45));
    let now = chrono::Utc::now().timestamp_millis();
    let reset = entry
        .reset_at_ms
        .expect("retry_after_ms should produce reset_at_ms");
    // Within reasonable jitter of now + 45s.
    assert!(
        (reset - (now + 45_000)).abs() < 1_000,
        "reset_at_ms should equal now + retry_after_ms (within 1s jitter); reset={reset} now={now}"
    );
}

#[tokio::test]
async fn record_rate_limit_observation_skips_empty_provider() {
    use crate::engine_helpers::record_rate_limit_observation;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    // Empty provider → skip silently rather than write a "" entry
    // that no selectivity check could match.
    record_rate_limit_observation(&app_state, "", ProviderApi::Anthropic, Some(1_000)).await;

    assert!(app_state.read().await.rate_limits.is_empty());
}

#[tokio::test]
async fn clear_rate_limit_observation_removes_no_reset_rejection() {
    use crate::engine_helpers::clear_rate_limit_observation;
    use crate::engine_helpers::record_rate_limit_observation;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    record_rate_limit_observation(&app_state, "anthropic", ProviderApi::Anthropic, None).await;
    assert!(
        app_state.read().await.rate_limits.contains_key("anthropic"),
        "no-reset rejection should be recorded"
    );

    clear_rate_limit_observation(&app_state, "anthropic").await;
    assert!(
        !app_state.read().await.rate_limits.contains_key("anthropic"),
        "successful provider call should clear stale rejection"
    );
}

#[tokio::test]
async fn build_suggestion_context_rate_limit_allowed_status_does_not_suppress() {
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    {
        let mut snap = app_state.write().await;
        let now = chrono::Utc::now().timestamp_millis();
        snap.rate_limits.insert(
            "anthropic".to_string(),
            RateLimitEntry {
                api: ProviderApi::Anthropic,
                status: RateLimitStatus::AllowedWarning,
                reset_at_ms: Some(now + 60_000),
                retry_after_seconds: None,
                last_observed_ms: now,
            },
        );
    }

    let cache = empty_cache("anthropic");
    let ctx = build_suggestion_context(&cache, &app_state, false, false).await;
    assert!(
        !ctx.rate_limit,
        "AllowedWarning should not suppress — only Rejected does"
    );
}
