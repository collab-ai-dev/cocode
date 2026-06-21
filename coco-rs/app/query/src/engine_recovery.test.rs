//! Regression tests for the post-stream recovery dispatcher.
//!
//! Plan completion criterion: regression tests for C15 (pre-API
//! blocking-limit gate), N1 (ModelInfo-aware max_output_tokens
//! escalate ceiling), and N2 (cache-break reset on cross-provider
//! advance).

use std::sync::Arc;

use coco_config::ModelInfo;
use coco_config::PositiveTokens;
use coco_inference::AISdkError;
use coco_inference::CacheBreakDetector;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::PrebuiltLanguageModelSlot;
use coco_inference::ProviderClientFingerprint;
use coco_inference::RetryConfig;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::LlmMessage;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::create_user_message;
use coco_tool_runtime::ToolRegistry;
use coco_types::ProviderModelSelection;
use coco_types::TokenUsage;
use coco_types::messages::AssistantMessage;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::BlockingLimitDecision;
use super::RecoveryDisposition;
use super::StreamErrorOutcome;
use crate::config::ContinueReason;
use crate::config::QueryEngineConfig;
use crate::engine::QueryEngine;
use crate::engine_loop_state::LoopServices;
use crate::engine_loop_state::LoopTurnState;
use crate::engine_stream_consume::WithheldReason;

// ──────────────────────────────────────────────────────────────────────
// Mock building blocks
// ──────────────────────────────────────────────────────────────────────

struct StubModel {
    provider: &'static str,
    id: &'static str,
}

#[async_trait::async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        self.provider
    }
    fn model_id(&self) -> &str {
        self.id
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "stub".into(),
                provider_metadata: None,
            })],
            usage: Usage::new(0, 0),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        Err(AISdkError::new("no stream"))
    }
}

fn slot_with_info(
    provider: &'static str,
    model_id: &'static str,
    info: ModelInfo,
) -> PrebuiltLanguageModelSlot {
    let model: Arc<dyn LanguageModel> = Arc::new(StubModel {
        provider,
        id: model_id,
    });
    let fingerprint = ProviderClientFingerprint {
        provider: provider.to_string(),
        api: coco_types::ProviderApi::OpenaiCompat,
        api_model_name: model_id.to_string(),
        base_url: String::new(),
        wire_api: None,
        client_options_digest: [0u8; 32],
        timeout_secs: 0,
        api_key_origin_digest: [0u8; 32],
        runtime_state_digest: [0u8; 32],
    };
    let identity = ProviderModelSelection {
        provider: provider.to_string(),
        model_id: model_id.to_string(),
    };
    PrebuiltLanguageModelSlot::new(model, RetryConfig::default())
        .with_fingerprint(fingerprint)
        .with_model_info(info)
        .with_model_identity(identity)
}

fn slot_default(provider: &'static str, model_id: &'static str) -> PrebuiltLanguageModelSlot {
    let model: Arc<dyn LanguageModel> = Arc::new(StubModel {
        provider,
        id: model_id,
    });
    PrebuiltLanguageModelSlot::new(model, RetryConfig::default())
}

/// Slot whose fingerprint reports the Anthropic wire API, so
/// `supports_server_side_context_edits()` returns true — required to exercise
/// the server-side reactive-compaction branch. (`slot_default` leaves the
/// fingerprint API unset, so it always takes the client-side branch regardless
/// of the provider string.)
fn slot_anthropic(model_id: &'static str) -> PrebuiltLanguageModelSlot {
    let model: Arc<dyn LanguageModel> = Arc::new(StubModel {
        provider: "anthropic",
        id: model_id,
    });
    let fingerprint = ProviderClientFingerprint {
        provider: "anthropic".to_string(),
        api: coco_types::ProviderApi::Anthropic,
        api_model_name: model_id.to_string(),
        base_url: String::new(),
        wire_api: None,
        client_options_digest: [0u8; 32],
        timeout_secs: 0,
        api_key_origin_digest: [0u8; 32],
        runtime_state_digest: [0u8; 32],
    };
    PrebuiltLanguageModelSlot::new(model, RetryConfig::default()).with_fingerprint(fingerprint)
}

fn registry_from_slot(slot: PrebuiltLanguageModelSlot) -> Arc<ModelRuntimeRegistry> {
    Arc::new(ModelRuntimeRegistry::from_prebuilt_language_model(
        coco_types::ModelRole::Main,
        slot,
    ))
}

fn slot_snapshot(slot: &PrebuiltLanguageModelSlot) -> coco_inference::ModelRuntimeSnapshot {
    registry_from_slot(slot.clone())
        .snapshot_for_role(coco_types::ModelRole::Main)
        .expect("snapshot")
}

fn test_engine(config: QueryEngineConfig, slot: PrebuiltLanguageModelSlot) -> QueryEngine {
    let tools = Arc::new(ToolRegistry::new());
    let cancel = CancellationToken::new();
    QueryEngine::new(config, registry_from_slot(slot), tools, cancel, None)
}

fn loop_turn_state() -> LoopTurnState {
    LoopTurnState::new(
        /*max_tokens*/ None,
        /*max_turns*/ Some(100),
        /*max_continuations*/ 3,
    )
}

fn assistant_partial(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant_text(text),
        uuid: Uuid::new_v4(),
        model: "test-model".into(),
        stop_reason: None,
        usage: Some(TokenUsage::default()),
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

/// Tracked `query_source` so `record_prompt_state` actually stores
/// a snapshot. `TRACKED_SOURCE_PREFIXES` in
/// `services/inference/src/cache_detection.rs` whitelist mirror.
const TRACKED_SOURCE: &str = "repl_main_thread";

// ──────────────────────────────────────────────────────────────────────
// C15 — pre-API blocking-limit gate
// ──────────────────────────────────────────────────────────────────────

/// C15 finding: an empty history must pass the gate. Sanity check
/// that the threshold math doesn't false-positive on the trivial
/// case.
#[tokio::test]
async fn c15_empty_history_proceeds() {
    let small = ModelInfo {
        context_window: PositiveTokens::new(10_000),
        max_output_tokens: PositiveTokens::new(4_096),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", small);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let history = MessageHistory::new();
    let turn_state = loop_turn_state();

    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Proceed => {}
        other => panic!("empty history should Proceed, got {other:?}"),
    }
}

/// C15 finding: when estimated history tokens exceed the
/// `context_window - reserved_output` threshold, the gate must
/// return `Block` so the engine can synthesize the
/// `blocking_limit` api_error rather than letting the request 4xx.
///
/// Sizing math (chars/4 estimator, 10_000-token context_window):
/// reserved = max(1024, 10_000 / 10) = 1024; threshold = 8_976.
/// We push a single User message whose serialized form exceeds the
/// threshold. 40_000 chars → ~10_000 estimated tokens > 8_976.
#[tokio::test]
async fn c15_overlimit_history_blocks() {
    let small = ModelInfo {
        context_window: PositiveTokens::new(10_000),
        max_output_tokens: PositiveTokens::new(4_096),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", small);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let huge_text = "x".repeat(40_000);
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));

    let turn_state = loop_turn_state();
    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Block {
            estimated_tokens,
            context_window,
        } => {
            assert_eq!(context_window, 10_000, "context_window from ModelInfo");
            assert!(
                estimated_tokens > 8_976,
                "estimated_tokens {estimated_tokens} must exceed (10_000 - 1024) threshold"
            );
        }
        other => panic!("overlimit history should Block, got {other:?}"),
    }
}

/// C15 finding: when the previous iteration ran reactive compaction,
/// the gate must skip — re-blocking after compaction would deadlock
/// the recovery loop (compact → block → compact → …).
#[tokio::test]
async fn c15_skips_post_compact() {
    let small = ModelInfo {
        context_window: PositiveTokens::new(10_000),
        max_output_tokens: PositiveTokens::new(4_096),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", small);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    // Same overlimit history as `c15_overlimit_history_blocks`.
    let huge_text = "x".repeat(40_000);
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));

    let mut turn_state = loop_turn_state();
    turn_state.transition = Some(ContinueReason::ReactiveCompactRetry);

    // R10 + R5 cleanup — `SkipPostCompact` collapsed into `Proceed`
    // (post-compact retry now reads as a `tracing::debug!` field
    // inside `check_blocking_limit` rather than a typed variant). The
    // caller's behavior is identical: proceed to `query_stream`.
    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Proceed => {}
        other => panic!("post-compact iteration must Proceed, got {other:?}"),
    }
}

/// C15 finding: when the active client has no `ModelInfo` wired
/// (test/mock paths), the gate falls back to the 200_000 default
/// rather than panicking on `None`. A typical history must pass.
#[tokio::test]
async fn c15_no_model_info_uses_default_window() {
    let client = slot_default("anthropic", "claude-3");
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let mut history = MessageHistory::new();
    history.push(create_user_message("hello"));

    let turn_state = loop_turn_state();
    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Proceed => {}
        other => panic!("default 200k window must Proceed for tiny history, got {other:?}"),
    }
}

// ──────────────────────────────────────────────────────────────────────
// MaxOutputTokens recovery — opt-in escalate via ModelInfo
// ──────────────────────────────────────────────────────────────────────
//
// Replaces the legacy N1 tests. The previous design used a single global
// `ESCALATED_MAX_TOKENS = 64_000` constant + a per-turn
// `max_tokens_override` slot that escalated unconditionally. That couldn't
// fit the multi-LLM architecture — Opus would escalate to 64k (useful),
// but GPT-4 (4096 cap) and Haiku (1024 cap) would get
// guaranteed-rejected requests. The new design puts the escalate
// ceiling on each `ModelInfo` (opt-in, per-model) and derives the
// "should escalate this turn?" decision from `turn_state.transition` —
// no stateful slot.

/// Phase-1 escalate fires when the active model opted in via
/// `ModelInfo.max_output_tokens_escalate` and the previous transition
/// wasn't already the escalate retry.
#[tokio::test]
async fn escalate_fires_when_model_opts_in() {
    // Mock model: baseline 4096, escalate ceiling 16384.
    let info = ModelInfo {
        context_window: PositiveTokens::new(128_000),
        max_output_tokens: PositiveTokens::new(4_096),
        max_output_tokens_escalate: Some(PositiveTokens::new(16_384)),
        ..Default::default()
    };
    let client = slot_with_info("openai", "gpt-4", info);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let mut history = MessageHistory::new();
    let mut turn_state = loop_turn_state();
    let event_tx = None;
    let assistant = assistant_partial("partial response");

    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::MaxOutputTokens,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&client),
        )
        .await;

    match disposition {
        RecoveryDisposition::Continue(ContinueReason::MaxOutputTokensEscalate) => {}
        other => panic!("opted-in model must escalate, got {other:?}"),
    }
    // The next iteration's `effective_max_tokens` reads the escalate
    // ceiling from ModelInfo via the transition match — no per-turn
    // state field involved.
    turn_state.transition = Some(ContinueReason::MaxOutputTokensEscalate);
    assert_eq!(
        super::effective_max_tokens(&slot_snapshot(&client), &turn_state),
        Some(16_384),
        "effective_max_tokens during escalate retry must return the ModelInfo ceiling",
    );
}

/// When the model does NOT declare `max_output_tokens_escalate`, Phase-1
/// is skipped entirely — recovery jumps straight to Phase-2 (resume
/// nudge). The opt-out path is the safe default for any provider whose
/// hard ceiling matches `max_output_tokens` (no escalate headroom).
#[tokio::test]
async fn escalate_skipped_when_model_does_not_opt_in() {
    // No `max_output_tokens_escalate` set — Phase-1 disabled.
    let info = ModelInfo {
        context_window: PositiveTokens::new(128_000),
        max_output_tokens: PositiveTokens::new(4_096),
        max_output_tokens_escalate: None,
        ..Default::default()
    };
    let client = slot_with_info("openai", "gpt-4", info);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let mut history = MessageHistory::new();
    let mut turn_state = loop_turn_state();
    let event_tx = None;
    let assistant = assistant_partial("partial");

    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::MaxOutputTokens,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&client),
        )
        .await;

    match disposition {
        RecoveryDisposition::Continue(ContinueReason::MaxOutputTokensRecovery { attempt: 1 }) => {}
        other => panic!("non-opted-in model must go straight to Phase-2, got {other:?}"),
    }
    assert_eq!(
        turn_state.max_tokens_recovery_count, 1,
        "Phase-2 must increment the recovery counter",
    );
}

/// Phase-1 must not fire twice in a row. After the escalate retry
/// itself hits MaxTokens, recovery falls through to Phase-2 — even on
/// opted-in models. Driven by `turn_state.transition` matching the
/// previous-iteration `MaxOutputTokensEscalate` reason.
#[tokio::test]
async fn escalate_not_re_entered_on_consecutive_max_tokens() {
    let info = ModelInfo {
        context_window: PositiveTokens::new(128_000),
        max_output_tokens: PositiveTokens::new(4_096),
        max_output_tokens_escalate: Some(PositiveTokens::new(16_384)),
        ..Default::default()
    };
    let client = slot_with_info("openai", "gpt-4", info);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let mut history = MessageHistory::new();
    let mut turn_state = loop_turn_state();
    // Simulate: the previous iteration already escalated; THIS iteration
    // is the retry that still ended with MaxTokens.
    turn_state.transition = Some(ContinueReason::MaxOutputTokensEscalate);
    let event_tx = None;
    let assistant = assistant_partial("partial after escalate");

    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::MaxOutputTokens,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&client),
        )
        .await;

    match disposition {
        RecoveryDisposition::Continue(ContinueReason::MaxOutputTokensRecovery { attempt: 1 }) => {}
        other => panic!("post-escalate MaxTokens must go to Phase-2, got {other:?}"),
    }
}

/// `effective_max_tokens` is the single source of truth for the per-call
/// `max_output_tokens`. Returns `None` on the normal path (defer to
/// `ModelInfo.max_output_tokens` at the inference seam) and `Some(N)`
/// for the one turn whose `transition` is the escalate retry.
#[tokio::test]
async fn effective_max_tokens_returns_none_outside_escalate_retry() {
    let info = ModelInfo {
        context_window: PositiveTokens::new(128_000),
        max_output_tokens: PositiveTokens::new(4_096),
        max_output_tokens_escalate: Some(PositiveTokens::new(16_384)),
        ..Default::default()
    };
    let client = slot_with_info("openai", "gpt-4", info);
    let mut turn_state = loop_turn_state();

    // Normal turn: no transition set → defer to ModelInfo.
    assert_eq!(
        super::effective_max_tokens(&slot_snapshot(&client), &turn_state),
        None
    );

    // Reactive compact retry: not the escalate path → still None.
    turn_state.transition = Some(ContinueReason::ReactiveCompactRetry);
    assert_eq!(
        super::effective_max_tokens(&slot_snapshot(&client), &turn_state),
        None
    );

    // Escalate retry: returns the ceiling.
    turn_state.transition = Some(ContinueReason::MaxOutputTokensEscalate);
    assert_eq!(
        super::effective_max_tokens(&slot_snapshot(&client), &turn_state),
        Some(16_384),
    );
}

/// When the model didn't opt in but transition somehow lands on
/// MaxOutputTokensEscalate (shouldn't happen in practice — the
/// dispatcher's gate prevents it), `effective_max_tokens` returns
/// `None`, falling through to the model baseline at the inference
/// seam. Defensive — proves the helper never invents a value.
#[tokio::test]
async fn effective_max_tokens_returns_none_when_opted_out() {
    let info = ModelInfo {
        context_window: PositiveTokens::new(128_000),
        max_output_tokens: PositiveTokens::new(4_096),
        max_output_tokens_escalate: None,
        ..Default::default()
    };
    let client = slot_with_info("openai", "gpt-4", info);
    let mut turn_state = loop_turn_state();
    turn_state.transition = Some(ContinueReason::MaxOutputTokensEscalate);

    assert_eq!(
        super::effective_max_tokens(&slot_snapshot(&client), &turn_state),
        None
    );
}

/// **H3 regression** — the recovery dispatcher reads its escalate
/// decision from the `active_client` parameter, not from any global
/// runtime slot. This matters under plan-mode swap: the engine routes
/// the turn through `plan_swap_candidate` (Plan role), and the
/// failure recovery must read THAT client's ModelInfo — not the Main
/// runtime client's.
///
/// Setup: simulate a plan-mode session where Main has an escalate
/// ceiling but the active (Plan-role) client does NOT. Recovery must
/// fall through to Phase-2, NOT fire a no-op Phase-1 against Main's
/// ceiling. The pre-fix bug fired Phase-1 then wasted a turn retrying
/// against the Plan client's baseline cap.
#[tokio::test]
async fn h3_recovery_reads_active_client_not_runtime_main() {
    // Main client: has escalate ceiling (Opus-class).
    let main_info = ModelInfo {
        context_window: PositiveTokens::new(200_000),
        max_output_tokens: PositiveTokens::new(16_384),
        max_output_tokens_escalate: Some(PositiveTokens::new(64_000)),
        ..Default::default()
    };
    let main_client = slot_with_info("anthropic", "opus", main_info);
    let engine = test_engine(QueryEngineConfig::default(), main_client.clone());

    // Plan client (the active client this turn): no escalate ceiling
    // (Haiku-class).
    let plan_info = ModelInfo {
        context_window: PositiveTokens::new(200_000),
        max_output_tokens: PositiveTokens::new(8_192),
        max_output_tokens_escalate: None,
        ..Default::default()
    };
    let plan_client = slot_with_info("anthropic", "haiku", plan_info);

    let mut history = MessageHistory::new();
    let mut turn_state = loop_turn_state();
    let event_tx = None;
    let assistant = assistant_partial("partial response from plan client");

    // Pass &plan_client — the engine.rs call site passes the
    // post-plan-swap client, so the dispatcher's escalate decision
    // tracks the model the next iteration's retry will actually hit.
    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::MaxOutputTokens,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&plan_client),
        )
        .await;

    match disposition {
        RecoveryDisposition::Continue(ContinueReason::MaxOutputTokensRecovery { attempt: 1 }) => {}
        other => panic!(
            "plan-mode swap must read plan client's (no escalate) ModelInfo and go to Phase-2, \
             got {other:?} — H3 regression"
        ),
    }
    // Sanity: had the dispatcher mistakenly read Main, it would have
    // returned Continue(MaxOutputTokensEscalate) (Main's ceiling 64k >
    // baseline 16k → phase1_available = true).
    assert_eq!(
        turn_state.max_tokens_recovery_count, 1,
        "Phase-2 must have incremented; Phase-1 was correctly skipped",
    );
}

// ──────────────────────────────────────────────────────────────────────
// N2 — cache-break reset on cross-provider advance
// ──────────────────────────────────────────────────────────────────────

/// N2 finding: when `ModelRuntime::advance` returns `Switched` and
/// crosses providers, the new slot's `CacheBreakDetector`
/// must be reset so it doesn't carry stale prompt-state hashes from
/// before the switch. `post_advance_side_effects` is the centralized
/// hook.
#[tokio::test]
async fn n2_post_advance_resets_cache_break_detector() {
    // Detector with one tracked state entry — proxy for "had cached
    // prompt history before the switch."
    let detector = Arc::new(Mutex::new(CacheBreakDetector::new()));
    let new_client = slot_default("openai", "gpt-4").with_cache_break_detector(detector.clone());

    // Pre-populate by running phase 1 of the detector.
    {
        let mut d = detector.lock().await;
        d.record_prompt_state(coco_inference::PromptStateInput {
            query_source: TRACKED_SOURCE.to_string(),
            model: "gpt-4".to_string(),
            ..Default::default()
        });
    }
    assert!(
        !detector.lock().await.is_empty(),
        "pre-condition: detector has tracked state",
    );

    let engine = test_engine(QueryEngineConfig::default(), new_client.clone());
    let services = loop_services(new_client);

    engine
        .post_advance_side_effects("anthropic", &services)
        .await;

    assert!(
        detector.lock().await.is_empty(),
        "post_advance_side_effects must reset the detector when called after a provider switch",
    );
}

/// N2 finding: even when the original provider matches the new
/// provider (within-provider advance), the dispatcher resets
/// conservatively — design choice from the recovery doc: the cost is
/// one extra Mutex lock per advance; the upside is no false-positive
/// cache breaks if any callee relied on the prior state.
#[tokio::test]
async fn n2_post_advance_resets_even_within_provider() {
    let detector = Arc::new(Mutex::new(CacheBreakDetector::new()));
    let new_client =
        slot_default("anthropic", "claude-3").with_cache_break_detector(detector.clone());

    {
        let mut d = detector.lock().await;
        d.record_prompt_state(coco_inference::PromptStateInput {
            query_source: TRACKED_SOURCE.to_string(),
            model: "claude-3".to_string(),
            ..Default::default()
        });
    }
    assert!(!detector.lock().await.is_empty());

    let engine = test_engine(QueryEngineConfig::default(), new_client.clone());
    let services = loop_services(new_client);

    // Same provider — no cross-provider log line, but reset still
    // fires (conservative invariant).
    engine
        .post_advance_side_effects("anthropic", &services)
        .await;

    assert!(
        detector.lock().await.is_empty(),
        "reset must fire even when provider is unchanged",
    );
}

// ──────────────────────────────────────────────────────────────────────
// A1 — handle_stream_open_error dispatcher
// ──────────────────────────────────────────────────────────────────────

/// Build a `LoopServices` whose `runtime` wraps `slot` with no
/// fallback slots. Used by the A1 handle_stream_open_error tests.
fn loop_services(slot: PrebuiltLanguageModelSlot) -> LoopServices {
    let (progress_tx, _progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<coco_tool_runtime::ToolProgress>();
    let registry = registry_from_slot(slot);
    let runtime = registry
        .runtime_for_role(coco_types::ModelRole::Main)
        .expect("runtime");
    LoopServices {
        runtime: runtime.clone(),
        runtime_source: coco_inference::ModelRuntimeSource::Role(coco_types::ModelRole::Main),
        main_runtime: runtime,
        main_source: coco_inference::ModelRuntimeSource::Role(coco_types::ModelRole::Main),
        progress_tx,
        plan: crate::plan_mode_reminder::PlanModeReminder::new(
            coco_types::PermissionMode::Default,
            None,
            None,
            None,
            None,
        ),
        reminders: coco_system_reminder::SystemReminderOrchestrator::new(
            coco_config::SystemReminderConfig::default(),
        ),
    }
}

fn engine_with_app_state(
    config: QueryEngineConfig,
    slot: PrebuiltLanguageModelSlot,
    app_state: Arc<tokio::sync::RwLock<coco_types::ToolAppState>>,
) -> QueryEngine {
    let tools = Arc::new(ToolRegistry::new());
    let cancel = CancellationToken::new();
    QueryEngine::new(config, registry_from_slot(slot), tools, cancel, None)
        .with_app_state(app_state)
}

/// A1 finding (stream-open path): a typed `Overloaded` error on a
/// primary-only runtime must surface as `Bail` after recording the
/// rate-limit observation onto `app_state.rate_limits` (so post-turn
/// forks see the throttle on the first 429).
#[tokio::test]
async fn a1_handle_stream_open_error_overloaded_records_observation() {
    let client = slot_default("openai", "gpt-4");
    let app_state = Arc::new(tokio::sync::RwLock::new(coco_types::ToolAppState::default()));
    let engine = engine_with_app_state(
        QueryEngineConfig::default(),
        client.clone(),
        app_state.clone(),
    );
    let mut services = loop_services(client.clone());
    let mut turn_state = loop_turn_state();
    let mut history = MessageHistory::new();

    let err = coco_inference::errors::OverloadedSnafu {
        retry_after_ms: Some(2_000_i64),
    }
    .build();

    let outcome = engine
        .handle_stream_open_error(
            err,
            &slot_snapshot(&client),
            Vec::new(),
            &mut services,
            &mut turn_state,
            &mut history,
            /*event_tx*/ &None,
        )
        .await;

    assert!(
        matches!(outcome, StreamErrorOutcome::Bail(_)),
        "primary-only capacity must Bail, got {outcome:?}",
    );
    let snap = app_state.read().await;
    let entry = snap
        .rate_limits
        .get("openai")
        .expect("rate-limit observation must be recorded for the active provider");
    assert_eq!(
        entry.retry_after_seconds,
        Some(2_i64),
        "provider-reported retry_after_ms must translate to retry_after_seconds",
    );
}

/// A1 finding (stream-open path): a generic non-capacity / non-overflow
/// error must surface as `Bail` so the outer loop returns `Err(_)`. The
/// observation map stays empty (no throttle to report) and capacity
/// bookkeeping is not touched.
#[tokio::test]
async fn a1_handle_stream_open_error_unrelated_error_bails() {
    let client = slot_default("openai", "gpt-4");
    let app_state = Arc::new(tokio::sync::RwLock::new(coco_types::ToolAppState::default()));
    let engine = engine_with_app_state(
        QueryEngineConfig::default(),
        client.clone(),
        app_state.clone(),
    );
    let mut services = loop_services(client.clone());
    let mut turn_state = loop_turn_state();
    let mut history = MessageHistory::new();

    let err = coco_inference::errors::AuthenticationFailedSnafu {
        message: "expired key".to_string(),
    }
    .build();

    let outcome = engine
        .handle_stream_open_error(
            err,
            &slot_snapshot(&client),
            Vec::new(),
            &mut services,
            &mut turn_state,
            &mut history,
            /*event_tx*/ &None,
        )
        .await;

    assert!(
        matches!(outcome, StreamErrorOutcome::Bail(_)),
        "auth failure must surface as Bail so the caller returns Err, got {outcome:?}",
    );
    let snap = app_state.read().await;
    assert!(
        snap.rate_limits.is_empty(),
        "non-capacity errors must NOT populate rate_limits",
    );
}

/// A1 finding (stream-open path): the typed `InferenceError` doesn't
/// catch every capacity surface — vercel-ai's retry layer occasionally
/// wraps Overloaded/RateLimited as a generic `ProviderError`. The
/// string-fallback via `is_capacity_error_message` must still drive
/// the dispatcher into the capacity branch so observation stays
/// accurate even on the un-typed path.
#[tokio::test]
async fn a1_handle_stream_open_error_capacity_string_fallback_still_records() {
    let client = slot_default("openai", "gpt-4");
    let app_state = Arc::new(tokio::sync::RwLock::new(coco_types::ToolAppState::default()));
    let engine = engine_with_app_state(
        QueryEngineConfig::default(),
        client.clone(),
        app_state.clone(),
    );
    let mut services = loop_services(client.clone());
    let mut turn_state = loop_turn_state();
    let mut history = MessageHistory::new();

    // Generic ProviderError whose message will trip
    // `is_capacity_error_message` (matches "overloaded_error").
    let err = coco_inference::errors::ProviderSnafu {
        status: 500_i32,
        message: "overloaded_error: provider returned 529".to_string(),
    }
    .build();

    let outcome = engine
        .handle_stream_open_error(
            err,
            &slot_snapshot(&client),
            Vec::new(),
            &mut services,
            &mut turn_state,
            &mut history,
            /*event_tx*/ &None,
        )
        .await;

    assert!(
        matches!(outcome, StreamErrorOutcome::Bail(_)),
        "primary-only string-fallback capacity must Bail, got {outcome:?}",
    );
    let snap = app_state.read().await;
    assert!(
        snap.rate_limits.contains_key("openai"),
        "string-fallback capacity must record an observation \
         (retry_after_ms is None on the string path)",
    );
}

/// A1 finding (stream-open path): with no fallback chain configured,
/// the dispatcher falls through to `Bail` after recording the
/// observation. Verifies the "no fallback" exit path is correctly
/// wired — without it, callers without a fallback chain would spin
/// forever on a saturated provider.
#[tokio::test]
async fn a1_handle_stream_open_error_capacity_without_fallback_bails() {
    let client = slot_default("openai", "gpt-4");
    let app_state = Arc::new(tokio::sync::RwLock::new(coco_types::ToolAppState::default()));
    let engine = engine_with_app_state(
        QueryEngineConfig::default(),
        client.clone(),
        app_state.clone(),
    );
    let mut services = loop_services(client.clone()); // no fallbacks
    let mut turn_state = loop_turn_state();
    let mut history = MessageHistory::new();

    let err = coco_inference::errors::OverloadedSnafu {
        retry_after_ms: None,
    }
    .build();

    let outcome = engine
        .handle_stream_open_error(
            err,
            &slot_snapshot(&client),
            Vec::new(),
            &mut services,
            &mut turn_state,
            &mut history,
            /*event_tx*/ &None,
        )
        .await;

    assert!(
        matches!(outcome, StreamErrorOutcome::Bail(_)),
        "capacity + no fallback must Bail, got {outcome:?}",
    );
    let snap = app_state.read().await;
    assert!(
        snap.rate_limits.contains_key("openai"),
        "observation must still be recorded on the Bail path so post-turn forks see the throttle",
    );
}

// ──────────────────────────────────────────────────────────────────────
// R1 — PromptTooLong recovery terminal exit on compact exhaustion
// ──────────────────────────────────────────────────────────────────────

/// R1 finding: when [`super::ContextOverflowOutcome::Exhausted`] is
/// reached (compaction circuit-breaker tripped + no progress),
/// `recover_prompt_too_long` MUST push the synthetic api_error message
/// tagged `prompt_too_long` and return
/// [`super::RecoveryDisposition::TerminateExhausted`] so the engine
/// falls through to the no-tool-calls terminal. Without this exit, the
/// outer loop would spin on the same overflowing prompt until
/// `BudgetTracker::Stop` fires.
///
/// We trip the circuit-breaker by recording 3 consecutive
/// [`coco_compact::ReactiveCompactState::record_failure`] entries
/// up-front; `do_reactive_compact`'s pre-check returns immediately
/// without doing any work and `handle_context_overflow` flags it as
/// Exhausted.
#[tokio::test]
async fn r1_recover_prompt_too_long_exhausted_pushes_synthetic_and_terminates() {
    let client = slot_default("anthropic", "claude-3");
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    // Trip the circuit-breaker before invoking recovery so
    // do_reactive_compact short-circuits and reports zero progress.
    {
        let mut state = engine.reactive_state.lock().await;
        state.record_failure(1);
        state.record_failure(2);
        state.record_failure(3);
        assert!(
            !state.should_attempt_reactive_compact(),
            "pre-condition: circuit breaker tripped",
        );
    }

    let mut history = MessageHistory::new();
    history.push(create_user_message("hello"));
    let mut turn_state = loop_turn_state();
    let event_tx = None;
    let assistant = assistant_partial("partial response before the wall");

    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::PromptTooLong,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&client),
        )
        .await;

    assert!(
        matches!(disposition, RecoveryDisposition::TerminateExhausted),
        "compact-exhausted PromptTooLong must surface TerminateExhausted, got {disposition:?}",
    );

    // The dispatcher must push BOTH the partial assistant_msg AND the
    // synthetic api_error tagged `prompt_too_long`. Walking from the
    // tail: synthetic api_error (last), then partial assistant_msg.
    let tail: Vec<_> = history.as_slice().iter().rev().take(2).collect();
    let synthetic = tail
        .first()
        .expect("history must contain synthetic api_error");
    match synthetic.as_ref() {
        coco_messages::Message::Assistant(a) => {
            let api_error = a.api_error.as_ref().expect("synthetic must carry ApiError");
            assert_eq!(
                api_error.error_type.as_deref(),
                Some("prompt_too_long"),
                "synthetic must be tagged so the C3 SkippedApiError handler \
                 exposes it as QueryResult.stop_reason",
            );
            assert!(
                api_error.message.contains("context window"),
                "synthetic must explain the cause, got {:?}",
                api_error.message,
            );
        }
        other => panic!("expected Assistant synthetic, got {other:?}"),
    }
    let partial = tail
        .get(1)
        .expect("history must contain partial assistant_msg");
    match partial.as_ref() {
        coco_messages::Message::Assistant(a) => {
            assert!(
                a.api_error.is_none(),
                "partial assistant_msg pushed alongside the synthetic must NOT \
                 itself carry api_error — the synthetic is the canonical marker",
            );
        }
        other => panic!("expected partial Assistant, got {other:?}"),
    }
}

// ──────────────────────────────────────────────────────────────────────
// R5 — C15 skips forked compact / session-memory agents
// ──────────────────────────────────────────────────────────────────────

/// R5 finding: a forked compact agent runs `run_session_loop` with
/// `query_source_override = Some("compact")`. Its whole purpose is to
/// shrink the oversized parent history; C15 MUST skip the gate so the
/// fork actually reaches the provider. TS parity: `query.ts:630-631`
/// `querySource !== 'compact'`.
#[tokio::test]
async fn r5_check_blocking_limit_skips_compact_fork_even_when_overlimit() {
    let small = ModelInfo {
        context_window: PositiveTokens::new(10_000),
        max_output_tokens: PositiveTokens::new(4_096),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", small);
    let config = QueryEngineConfig {
        query_source_override: Some("compact".into()),
        ..Default::default()
    };
    let engine = test_engine(config, client.clone());

    // Same overlimit history shape as `c15_overlimit_history_blocks`.
    let huge_text = "x".repeat(40_000);
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));
    let turn_state = loop_turn_state();

    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Proceed => {}
        other => panic!(
            "compact fork must Proceed even when overlimit — the fork EXISTS to \
             shrink this history, got {other:?}"
        ),
    }
}

/// R5 finding: session-memory forks also skip. The label set in
/// `is_forked_compact_or_session_memory_source` covers `session_memory`
/// (TS bare label) plus coco-rs additions `session_memory_auto` /
/// `session_memory_manual` / `extract_memories`.
#[tokio::test]
async fn r5_check_blocking_limit_skips_session_memory_fork() {
    let small = ModelInfo {
        context_window: PositiveTokens::new(10_000),
        max_output_tokens: PositiveTokens::new(4_096),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", small);
    for label in [
        "session_memory",
        "session_memory_auto",
        "session_memory_manual",
        "extract_memories",
    ] {
        let config = QueryEngineConfig {
            query_source_override: Some(label.into()),
            ..Default::default()
        };
        let engine = test_engine(config, client.clone());

        let huge_text = "x".repeat(40_000);
        let mut history = MessageHistory::new();
        history.push(create_user_message(&huge_text));
        let turn_state = loop_turn_state();

        match engine.check_blocking_limit(
            &history,
            &slot_snapshot(&client),
            &turn_state,
            /*effective_max_tokens*/ None,
        ) {
            BlockingLimitDecision::Proceed => {}
            other => panic!("query_source={label} must Proceed, got {other:?}"),
        }
    }
}

/// R5 finding (anti-test): non-compact forks (prompt_suggestion, …)
/// should NOT skip — those operate on already-fitting history and the
/// gate is still a useful guard against unexpected overflow.
#[tokio::test]
async fn r5_check_blocking_limit_does_not_skip_prompt_suggestion_fork() {
    let small = ModelInfo {
        context_window: PositiveTokens::new(10_000),
        max_output_tokens: PositiveTokens::new(4_096),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", small);
    let config = QueryEngineConfig {
        query_source_override: Some("prompt_suggestion".into()),
        ..Default::default()
    };
    let engine = test_engine(config, client.clone());

    let huge_text = "x".repeat(40_000);
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));
    let turn_state = loop_turn_state();

    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Block { .. } => {}
        other => panic!("prompt_suggestion fork must still honor the gate, got {other:?}"),
    }
}

// ──────────────────────────────────────────────────────────────────────
// R8 — C15 reserved_output tracks effective_max_tokens, not 10%
// ──────────────────────────────────────────────────────────────────────

/// R8 finding: when caller supplies `effective_max_tokens`, the
/// threshold is `context_window - effective_max_tokens` — matching the
/// provider's enforcement of `prompt + max_tokens ≤ window`. Without
/// this, the legacy 10% heuristic on a 200k window reserves only 20k
/// and lets requests through that the provider rejects.
///
/// Sizing math: 200k window with `effective_max_tokens = 64_000` →
/// threshold = 136_000. A history estimating ~140k tokens (560k chars)
/// must Block.
#[tokio::test]
async fn r8_check_blocking_limit_uses_effective_max_tokens_threshold() {
    let big = ModelInfo {
        context_window: PositiveTokens::new(200_000),
        max_output_tokens: PositiveTokens::new(64_000),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", big);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    // 560_000 chars / 4 = ~140k estimated tokens. Above 136k threshold
    // (200k - 64k) → Block. Below 180k (200k - 20k, legacy 10% gate) →
    // would have Proceeded.
    let huge_text = "x".repeat(560_000);
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));
    let turn_state = loop_turn_state();

    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ Some(64_000),
    ) {
        BlockingLimitDecision::Block {
            estimated_tokens,
            context_window,
        } => {
            assert_eq!(context_window, 200_000);
            assert!(
                estimated_tokens > 136_000,
                "estimated_tokens {estimated_tokens} must exceed (200k - 64k) threshold",
            );
        }
        other => panic!(
            "with effective_max_tokens=64k on a 200k window, ~140k prompt must Block, \
             got {other:?}"
        ),
    }
}

/// R8 cross-check: with `effective_max_tokens = None` BUT a wired
/// `ModelInfo` carrying the baseline `max_output_tokens`, the gate
/// reads the baseline directly — that's what the provider will actually
/// enforce on a non-escalate call. Threshold matches the provider's
/// real `prompt + max_tokens ≤ window` rule rather than the heuristic.
///
/// Same shape as the "with effective_max_tokens" overlimit case:
/// 200k window, 64k baseline ⇒ threshold 136k. A ~140k prompt must
/// Block on both paths because the provider would 4xx either way.
#[tokio::test]
async fn r8_check_blocking_limit_falls_back_to_model_info_baseline() {
    let info = ModelInfo {
        context_window: PositiveTokens::new(200_000),
        max_output_tokens: PositiveTokens::new(64_000),
        ..Default::default()
    };
    let client = slot_with_info("anthropic", "claude-3", info);
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let huge_text = "x".repeat(560_000); // ~140k tokens
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));
    let turn_state = loop_turn_state();

    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Block {
            estimated_tokens,
            context_window,
        } => {
            assert_eq!(context_window, 200_000);
            assert!(
                estimated_tokens > 200_000 - 64_000,
                "with ModelInfo.max_output_tokens=64k the threshold is 136k; \
                 a 140k prompt must Block",
            );
        }
        other => {
            panic!("ModelInfo baseline tier must clamp to 136k threshold and Block, got {other:?}",)
        }
    }
}

/// R8 ultimate fallback: when NO `ModelInfo` is wired (test paths /
/// mock clients) AND `effective_max_tokens` is None, the gate falls
/// back to the 10% heuristic. Validates the bottom of the three-tier
/// resolution chain stays intact.
#[tokio::test]
async fn r8_check_blocking_limit_falls_back_to_10pct_without_model_info() {
    // `slot_default` builds a runtime slot with NO ModelInfo wired,
    // so `model_info()` returns None.
    let client = slot_default("anthropic", "claude-3");
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let huge_text = "x".repeat(560_000); // ~140k tokens
    let mut history = MessageHistory::new();
    history.push(create_user_message(&huge_text));
    let turn_state = loop_turn_state();

    // `check_blocking_limit` uses `DEFAULT_CONTEXT_WINDOW = 200k` when
    // ModelInfo is absent, then reserved = max(1024, 200k/10) = 20k,
    // threshold = 180k. 140k < 180k → Proceed.
    match engine.check_blocking_limit(
        &history,
        &slot_snapshot(&client),
        &turn_state,
        /*effective_max_tokens*/ None,
    ) {
        BlockingLimitDecision::Proceed => {}
        other => panic!(
            "fallback 10% heuristic on 200k default window must Proceed for 140k prompt, \
             got {other:?}"
        ),
    }
}

/// R1 finding (happy path): when compaction succeeds (circuit-breaker
/// NOT tripped, history shrinks), the dispatcher returns Continue and
/// does NOT push a synthetic api_error. Cross-check: the new
/// TerminateExhausted exit must not regress the existing happy path.
#[tokio::test]
async fn r1_recover_prompt_too_long_compacted_keeps_continue_disposition() {
    let client = slot_anthropic("claude-3");
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    // Circuit-breaker NOT tripped — fresh state. The stub client is
    // provider="anthropic", so do_reactive_compact takes the server-side
    // branch: it QUEUES a `context_management` payload (no local history
    // mutation) and reports progress — the cache-preserving retry will carry
    // the payload and the API clears in place. This is the happy path the
    // test name promises: the turn continues, it is not terminated.

    let mut history = MessageHistory::new();
    history.push(create_user_message("hello"));
    let mut turn_state = loop_turn_state();
    let event_tx = None;
    let assistant = assistant_partial("partial response");

    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::PromptTooLong,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&client),
        )
        .await;

    // Anthropic supports server-side context edits, so do_reactive_compact
    // queues a payload and reports progress: the dispatcher must Continue with
    // ReactiveCompactRetry, NOT terminate. (Before the L3 fix this path
    // spuriously returned TerminateExhausted because progress was measured by
    // freed LOCAL tokens, which is always zero on the server-side branch.)
    assert!(
        matches!(
            disposition,
            RecoveryDisposition::Continue(ContinueReason::ReactiveCompactRetry)
        ),
        "anthropic server-side reactive recovery must Continue(ReactiveCompactRetry), \
         got {disposition:?}",
    );

    // Happy path: no synthetic prompt_too_long api_error is pushed — the turn
    // is retried with the queued context_management, not terminated.
    let synthetic_present = history.as_slice().iter().rev().take(2).any(|m| {
        matches!(
            m.as_ref(),
            coco_messages::Message::Assistant(a)
                if a.api_error
                    .as_ref()
                    .and_then(|e| e.error_type.as_deref())
                    == Some("prompt_too_long")
        )
    });
    assert!(
        !synthetic_present,
        "server-side reactive recovery should retry, not push a terminal api_error",
    );
}

/// L3 cross-check: on the CLIENT-SIDE branch (non-Anthropic), a reactive
/// attempt that frees no local tokens (tiny history, nothing to clear) makes
/// no progress → the dispatcher must TerminateExhausted and push the synthetic
/// api_error so the C3 StopFailure guard fires next iteration. This preserves
/// the zero-progress coverage after the happy-path test moved to the
/// server-side branch.
#[tokio::test]
async fn r1_recover_prompt_too_long_client_side_zero_progress_exhausts() {
    let client = slot_default("openai", "gpt-4");
    let engine = test_engine(QueryEngineConfig::default(), client.clone());

    let mut history = MessageHistory::new();
    history.push(create_user_message("hello"));
    let mut turn_state = loop_turn_state();
    let event_tx = None;
    let assistant = assistant_partial("partial response");

    let disposition = engine
        .run_post_stream_recovery(
            WithheldReason::PromptTooLong,
            assistant,
            &mut history,
            &event_tx,
            &mut turn_state,
            &slot_snapshot(&client),
        )
        .await;

    assert!(
        matches!(disposition, RecoveryDisposition::TerminateExhausted),
        "client-side zero-progress reactive recovery must TerminateExhausted, got {disposition:?}",
    );
    let synthetic_present = history.as_slice().iter().rev().take(2).any(|m| {
        matches!(
            m.as_ref(),
            coco_messages::Message::Assistant(a)
                if a.api_error
                    .as_ref()
                    .and_then(|e| e.error_type.as_deref())
                    == Some("prompt_too_long")
        )
    });
    assert!(
        synthetic_present,
        "TerminateExhausted must push the synthetic api_error for the C3 StopFailure guard",
    );
}
