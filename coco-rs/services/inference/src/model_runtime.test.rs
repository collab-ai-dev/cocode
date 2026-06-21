//! Tests for [`ModelRuntime`].
//!
//! Covers multi-slot chain degradation, the I13 hook invariant, and
//! the half-open probe state machine (backoff ramp, attempts cap,
//! deep-chain revert, owned probe state).

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::LanguageModel;
use crate::LanguageModelCallOptions;
use crate::LanguageModelGenerateResult;
use crate::LanguageModelStreamResult;
use crate::QueryParams;
use crate::RetryConfig;
use crate::client::ApiClient;
use coco_config::FallbackPolicy;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use pretty_assertions::assert_eq;

use super::*;

struct StubModel {
    id: &'static str,
}

#[async_trait::async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }
    fn model_id(&self) -> &str {
        self.id
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, crate::AISdkError> {
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
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, crate::AISdkError> {
        Ok(crate::stream::synthetic_stream_from_content(
            vec![AssistantContentPart::Text(TextPart {
                text: "stub".into(),
                provider_metadata: None,
            })],
            Usage::new(0, 0),
            FinishReason::new(StopReason::EndTurn),
        ))
    }
}

struct CapacityErrorModel {
    id: &'static str,
}

#[async_trait::async_trait]
impl LanguageModel for CapacityErrorModel {
    fn provider(&self) -> &str {
        "stub"
    }
    fn model_id(&self) -> &str {
        self.id
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, crate::AISdkError> {
        Err(crate::AISdkError::new("status: 503 provider overloaded"))
    }
    async fn do_stream(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, crate::AISdkError> {
        Err(crate::AISdkError::new("status: 503 provider overloaded"))
    }
}

fn stub_client(id: &'static str) -> Arc<ApiClient> {
    Arc::new(ApiClient::with_default_fingerprint(
        Arc::new(StubModel { id }),
        RetryConfig::default(),
    ))
}

fn capacity_client(id: &'static str) -> Arc<ApiClient> {
    Arc::new(ApiClient::with_default_fingerprint(
        Arc::new(CapacityErrorModel { id }),
        RetryConfig::default(),
    ))
}

/// Test-only convenience builder: equivalent to the deleted
/// `primary_only` / `with_fallback` shims.
fn with_single_fallback(primary: &'static str, fallback: &'static str) -> ModelRuntime {
    ModelRuntime::new(stub_client(primary), vec![stub_client(fallback)])
}

fn primary_only(primary: &'static str) -> ModelRuntime {
    ModelRuntime::new(stub_client(primary), Vec::new())
}

fn fast_policy() -> FallbackPolicy {
    // 1s initial → 4s cap, 3 attempts — keeps test doubling
    // observable in just a few iterations.
    FallbackPolicy {
        recovery: coco_config::RecoveryProbePolicy {
            initial_backoff_secs: 1,
            max_backoff_secs: 4,
            max_attempts: 3,
        },
        ..Default::default()
    }
}

fn immediate_policy() -> FallbackPolicy {
    FallbackPolicy {
        recovery: coco_config::RecoveryProbePolicy {
            initial_backoff_secs: 0,
            max_backoff_secs: 1,
            max_attempts: 3,
        },
        ..Default::default()
    }
}

fn role_token(rt: &ModelRuntime, role: coco_types::ModelRole) -> ModelCallHandle {
    ModelCallHandle {
        runtime: None,
        source: ModelRuntimeSource::Role(role),
        runtime_id: rt.instance_id,
        generation: rt.generation,
        slot_index: rt.active,
    }
}

fn explicit_token(rt: &ModelRuntime, provider: &str, model_id: &str) -> ModelCallHandle {
    ModelCallHandle {
        runtime: None,
        source: ModelRuntimeSource::Explicit(coco_types::ProviderModelSelection {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        }),
        runtime_id: rt.instance_id,
        generation: rt.generation,
        slot_index: rt.active,
    }
}

fn probe_params() -> QueryParams {
    recovery_probe_params()
}

fn extract_text(result: &QueryResult) -> String {
    result
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

// ─── Basic slot-state tests ─────────────────────────────────────────────────

#[test]
fn test_primary_only_reports_primary_model_id() {
    let rt = primary_only("primary");
    assert_eq!(rt.current_model_id(), "primary");
    assert_eq!(rt.active_index(), 0);
    assert!(!rt.has_fallback());
}

#[test]
fn test_advance_without_fallback_reports_exhausted() {
    let mut rt = primary_only("primary");
    assert_eq!(rt.advance(), AdvanceOutcome::Exhausted);
    assert_eq!(rt.active_index(), 0, "exhausted must not mutate active");
}

#[test]
fn test_advance_single_fallback_then_exhausted() {
    let mut rt = with_single_fallback("primary", "fallback");
    assert_eq!(rt.current_model_id(), "primary");
    assert_eq!(rt.advance(), AdvanceOutcome::Switched("fallback".into()));
    assert_eq!(rt.active_index(), 1);
    assert_eq!(rt.current_model_id(), "fallback");
    assert_eq!(
        rt.advance(),
        AdvanceOutcome::Exhausted,
        "second advance runs out of slots"
    );
    assert_eq!(rt.active_index(), 1, "exhausted must not over-advance");
}

#[test]
fn test_new_walks_every_slot_in_order() {
    let mut rt = ModelRuntime::new(
        stub_client("primary"),
        vec![stub_client("fb1"), stub_client("fb2"), stub_client("fb3")],
    );
    assert_eq!(rt.slot_count(), 4);
    assert_eq!(rt.current_model_id(), "primary");
    assert_eq!(rt.advance(), AdvanceOutcome::Switched("fb1".into()));
    assert_eq!(rt.advance(), AdvanceOutcome::Switched("fb2".into()));
    assert_eq!(rt.advance(), AdvanceOutcome::Switched("fb3".into()));
    assert_eq!(rt.advance(), AdvanceOutcome::Exhausted);
}

#[test]
fn test_new_with_empty_fallbacks_is_primary_only() {
    let rt = ModelRuntime::new(stub_client("primary"), Vec::new());
    assert_eq!(rt.slot_count(), 1);
    assert!(!rt.has_fallback());
}

#[test]
fn test_current_client_returns_active_slot() {
    let mut rt = ModelRuntime::new(
        stub_client("primary"),
        vec![stub_client("fb1"), stub_client("fb2")],
    );
    assert_eq!(rt.current_client().model_id(), "primary");
    rt.advance();
    assert_eq!(rt.current_client().model_id(), "fb1");
    rt.advance();
    assert_eq!(rt.current_client().model_id(), "fb2");
}

#[test]
fn test_finish_call_single_capacity_error_switches_fallback() {
    let mut rt = with_single_fallback("primary", "fallback");
    let token = role_token(&rt, coco_types::ModelRole::Main);

    let events = rt.finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );

    assert_eq!(rt.current_model_id(), "fallback");
    assert_eq!(
        events,
        vec![ModelRuntimeEvent::FallbackSwitched {
            source: ModelRuntimeSource::Role(coco_types::ModelRole::Main),
            from_model_id: "primary".to_string(),
            to_model_id: "fallback".to_string(),
        }]
    );
}

#[test]
fn test_finish_call_success_does_not_prevent_next_capacity_switch() {
    let mut rt = with_single_fallback("primary", "fallback");
    let token = role_token(&rt, coco_types::ModelRole::Main);

    assert!(
        rt.finish_call(&token, ModelCommunicationOutcome::Success)
            .is_empty()
    );
    let events = rt.finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );

    assert_eq!(rt.current_model_id(), "fallback");
    assert!(matches!(
        events.as_slice(),
        [ModelRuntimeEvent::FallbackSwitched { .. }]
    ));
}

#[test]
fn test_chain_exhaustion_retries_primary_until_max_cycles() {
    let mut rt = with_single_fallback("primary", "fallback").with_policy(FallbackPolicy {
        exhausted_retry: coco_config::ExhaustedRetryPolicy {
            max_cycles: 2,
            initial_backoff_secs: 0,
            max_backoff_secs: 0,
        },
        ..Default::default()
    });
    let token = role_token(&rt, coco_types::ModelRole::Main);
    let events = rt.finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );
    assert!(matches!(
        events.as_slice(),
        [ModelRuntimeEvent::FallbackSwitched { .. }]
    ));

    let token = role_token(&rt, coco_types::ModelRole::Main);
    assert!(
        rt.finish_call(
            &token,
            ModelCommunicationOutcome::Capacity {
                retry_after_ms: None,
            },
        )
        .is_empty(),
        "first exhausted cycle resets to primary without caller-facing event",
    );
    assert_eq!(rt.current_model_id(), "primary");

    let token = role_token(&rt, coco_types::ModelRole::Main);
    let events = rt.finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );
    assert!(matches!(
        events.as_slice(),
        [ModelRuntimeEvent::FallbackSwitched { .. }]
    ));

    let token = role_token(&rt, coco_types::ModelRole::Main);
    assert!(
        rt.finish_call(
            &token,
            ModelCommunicationOutcome::Capacity {
                retry_after_ms: None,
            },
        )
        .is_empty(),
        "final exhaustion surfaces the original error without an event",
    );
    assert_eq!(rt.current_model_id(), "fallback");
}

#[tokio::test]
async fn test_query_once_retry_chain_returns_retry_without_reusing_params() {
    let runtime = Arc::new(std::sync::Mutex::new(
        ModelRuntime::new(
            capacity_client("primary"),
            vec![capacity_client("fallback")],
        )
        .with_policy(FallbackPolicy {
            exhausted_retry: coco_config::ExhaustedRetryPolicy {
                max_cycles: 2,
                initial_backoff_secs: 0,
                max_backoff_secs: 0,
            },
            ..Default::default()
        }),
    ));
    let source = ModelRuntimeSource::Role(coco_types::ModelRole::Main);

    let first = ModelRuntime::query_once(runtime.clone(), source.clone(), &probe_params()).await;
    assert!(matches!(
        first,
        ModelRuntimeQueryOutcome::Retry { ref events }
            if matches!(events.as_slice(), [ModelRuntimeEvent::FallbackSwitched { .. }])
    ));
    assert_eq!(mutex_lock(&runtime).current_model_id(), "fallback");

    let second = ModelRuntime::query_once(runtime.clone(), source, &probe_params()).await;
    assert!(matches!(
        second,
        ModelRuntimeQueryOutcome::Retry { ref events } if events.is_empty()
    ));
    assert_eq!(
        mutex_lock(&runtime).current_model_id(),
        "primary",
        "exhausted-chain retry must return to the caller after backoff"
    );
}

#[tokio::test]
async fn test_client_query_with_rebuild_invokes_builder_after_retry() {
    let registry = Arc::new(ModelRuntimeRegistry::from_prebuilt_language_models(
        coco_types::ModelRole::Main,
        PrebuiltLanguageModelSlot::new(
            Arc::new(CapacityErrorModel { id: "primary" }),
            RetryConfig::default(),
        ),
        vec![PrebuiltLanguageModelSlot::new(
            Arc::new(StubModel { id: "fallback" }),
            RetryConfig::default(),
        )],
    ));
    let client = ModelRuntimeClient::new(
        registry,
        ModelRuntimeSource::Role(coco_types::ModelRole::Main),
    );
    let build_count = AtomicUsize::new(0);
    let mut seen_models = Vec::new();

    let result = client
        .query_with_rebuild(|snapshot| {
            build_count.fetch_add(1, Ordering::SeqCst);
            seen_models.push(snapshot.model_id.clone());
            probe_params()
        })
        .await
        .expect("fallback should succeed");

    assert_eq!(extract_text(&result), "stub");
    assert_eq!(build_count.load(Ordering::SeqCst), 2);
    assert_eq!(seen_models, ["primary", "fallback"]);
}

#[tokio::test]
async fn test_client_open_stream_with_rebuild_invokes_builder_after_retry() {
    let registry = Arc::new(ModelRuntimeRegistry::from_prebuilt_language_models(
        coco_types::ModelRole::Main,
        PrebuiltLanguageModelSlot::new(
            Arc::new(CapacityErrorModel { id: "primary" }),
            RetryConfig::default(),
        ),
        vec![PrebuiltLanguageModelSlot::new(
            Arc::new(StubModel { id: "fallback" }),
            RetryConfig::default(),
        )],
    ));
    let client = ModelRuntimeClient::new(
        registry,
        ModelRuntimeSource::Role(coco_types::ModelRole::Main),
    );
    let build_count = AtomicUsize::new(0);
    let mut seen_models = Vec::new();

    let (_rx, _token) = client
        .open_stream_with_rebuild(|snapshot| {
            build_count.fetch_add(1, Ordering::SeqCst);
            seen_models.push(snapshot.model_id.clone());
            probe_params()
        })
        .await
        .expect("fallback stream should open");

    assert_eq!(build_count.load(Ordering::SeqCst), 2);
    assert_eq!(seen_models, ["primary", "fallback"]);
}

#[test]
fn test_finish_call_ignores_stale_generation_token() {
    let mut rt = with_single_fallback("primary", "fallback");
    assert_eq!(rt.advance(), AdvanceOutcome::Switched("fallback".into()));
    let stale_token = ModelCallHandle {
        runtime: None,
        source: ModelRuntimeSource::Role(coco_types::ModelRole::Main),
        runtime_id: rt.instance_id,
        generation: 0,
        slot_index: 0,
    };

    for _ in 0..3 {
        assert!(
            rt.finish_call(
                &stale_token,
                ModelCommunicationOutcome::Capacity {
                    retry_after_ms: None,
                },
            )
            .is_empty()
        );
    }

    assert_eq!(rt.current_model_id(), "fallback");
}

#[test]
fn test_finish_call_ignores_stale_slot_token() {
    let mut rt = with_single_fallback("primary", "fallback");
    let wrong_slot_token = ModelCallHandle {
        runtime: None,
        source: ModelRuntimeSource::Role(coco_types::ModelRole::Main),
        runtime_id: rt.instance_id,
        generation: 0,
        slot_index: 1,
    };

    for _ in 0..3 {
        assert!(
            rt.finish_call(
                &wrong_slot_token,
                ModelCommunicationOutcome::Capacity {
                    retry_after_ms: None,
                },
            )
            .is_empty()
        );
    }

    assert_eq!(rt.current_model_id(), "primary");
}

#[test]
fn test_finish_call_ignores_stale_runtime_token() {
    let old = with_single_fallback("old-primary", "old-fallback");
    let mut new = with_single_fallback("new-primary", "new-fallback");
    let stale_token = role_token(&old, coco_types::ModelRole::Main);

    for _ in 0..3 {
        assert!(
            new.finish_call(
                &stale_token,
                ModelCommunicationOutcome::Capacity {
                    retry_after_ms: None,
                },
            )
            .is_empty()
        );
    }

    assert_eq!(new.current_model_id(), "new-primary");
}

#[tokio::test]
async fn test_public_finish_call_uses_opened_runtime_not_rebound_source() {
    let registry = Arc::new(ModelRuntimeRegistry::from_prebuilt_role_runtimes([(
        coco_types::ModelRole::Main,
        with_single_fallback("old-primary", "old-fallback"),
    )]));
    let old_runtime = registry
        .runtime_for_role(coco_types::ModelRole::Main)
        .expect("old runtime");
    let opened = registry
        .open_stream(
            ModelRuntimeSource::Role(coco_types::ModelRole::Main),
            &probe_params(),
        )
        .await;
    let token = match opened {
        ModelStreamOpenOutcome::Opened { token, .. } => token,
        _ => panic!("old runtime should open stream"),
    };

    let new_runtime = Arc::new(std::sync::Mutex::new(with_single_fallback(
        "new-primary",
        "new-fallback",
    )));
    rw_write(&registry.role_runtimes).insert(coco_types::ModelRole::Main, new_runtime.clone());

    let events = registry.finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );
    assert!(events.is_empty());
    assert_eq!(mutex_lock(&old_runtime).current_model_id(), "old-primary");
    assert_eq!(mutex_lock(&new_runtime).current_model_id(), "new-primary");
}

#[tokio::test]
async fn test_finish_call_for_retry_maps_stale_capacity_to_retry_current() {
    let registry = Arc::new(ModelRuntimeRegistry::from_prebuilt_role_runtimes([(
        coco_types::ModelRole::Main,
        with_single_fallback("primary", "fallback"),
    )]));
    let runtime = registry
        .runtime_for_role(coco_types::ModelRole::Main)
        .expect("main runtime");
    let stale_token = {
        let mut rt = mutex_lock(&runtime);
        let mut token = role_token(&rt, coco_types::ModelRole::Main);
        token.runtime = Some(runtime.clone());
        assert_eq!(rt.advance(), AdvanceOutcome::Switched("fallback".into()));
        token
    };

    let outcome = registry
        .finish_call_for_retry(
            &stale_token,
            ModelCommunicationOutcome::Capacity {
                retry_after_ms: None,
            },
        )
        .await;

    assert!(matches!(
        outcome,
        ModelRuntimeFeedbackOutcome::Retry { ref events } if events.is_empty()
    ));
    assert_eq!(mutex_lock(&runtime).current_model_id(), "fallback");
}

#[tokio::test]
async fn test_finish_call_for_retry_maps_rebound_capacity_to_retry_current() {
    let registry = Arc::new(ModelRuntimeRegistry::from_prebuilt_role_runtimes([(
        coco_types::ModelRole::Main,
        with_single_fallback("old-primary", "old-fallback"),
    )]));
    let opened = registry
        .open_stream(
            ModelRuntimeSource::Role(coco_types::ModelRole::Main),
            &probe_params(),
        )
        .await;
    let token = match opened {
        ModelStreamOpenOutcome::Opened { token, .. } => token,
        _ => panic!("old runtime should open stream"),
    };

    let new_runtime = Arc::new(std::sync::Mutex::new(with_single_fallback(
        "new-primary",
        "new-fallback",
    )));
    rw_write(&registry.role_runtimes).insert(coco_types::ModelRole::Main, new_runtime.clone());

    let outcome = registry
        .finish_call_for_retry(
            &token,
            ModelCommunicationOutcome::Capacity {
                retry_after_ms: None,
            },
        )
        .await;

    assert!(matches!(
        outcome,
        ModelRuntimeFeedbackOutcome::Retry { ref events } if events.is_empty()
    ));
    assert_eq!(mutex_lock(&new_runtime).current_model_id(), "new-primary");
}

#[tokio::test]
async fn test_open_stream_success_then_capacity_switches_fallback() {
    let runtime = Arc::new(std::sync::Mutex::new(with_single_fallback(
        "primary", "fallback",
    )));

    let opened = ModelRuntime::open_stream(
        runtime.clone(),
        ModelRuntimeSource::Role(coco_types::ModelRole::Main),
        &probe_params(),
    )
    .await;
    let token = match opened {
        ModelStreamOpenOutcome::Opened { token, .. } => token,
        _ => panic!("stream should open"),
    };
    let events = mutex_lock(&runtime).finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );

    assert_eq!(mutex_lock(&runtime).current_model_id(), "fallback");
    assert!(matches!(
        events.as_slice(),
        [ModelRuntimeEvent::FallbackSwitched { .. }]
    ));
}

#[test]
fn test_explicit_primary_only_capacity_does_not_fallback() {
    let mut rt = primary_only("explicit");
    let token = explicit_token(&rt, "stub", "explicit");

    for _ in 0..4 {
        assert!(
            rt.finish_call(
                &token,
                ModelCommunicationOutcome::Capacity {
                    retry_after_ms: None,
                },
            )
            .is_empty()
        );
    }

    assert_eq!(rt.current_model_id(), "explicit");
    assert_eq!(rt.active_index(), 0);
}

#[tokio::test]
async fn test_registry_recovery_probe_success_switches_back_to_primary() {
    let runtime = with_single_fallback("primary", "fallback").with_policy(immediate_policy());
    let registry = Arc::new(ModelRuntimeRegistry::from_prebuilt_role_runtimes([(
        coco_types::ModelRole::Main,
        runtime,
    )]));
    let runtime = registry
        .runtime_for_role(coco_types::ModelRole::Main)
        .expect("main runtime");
    let token = {
        let rt = mutex_lock(&runtime);
        let mut token = role_token(&rt, coco_types::ModelRole::Main);
        token.runtime = Some(runtime.clone());
        token
    };
    let mut events_rx = registry.subscribe_events();

    registry.finish_call(
        &token,
        ModelCommunicationOutcome::Capacity {
            retry_after_ms: None,
        },
    );
    assert_eq!(mutex_lock(&runtime).current_model_id(), "fallback");

    let mut recovered = false;
    for _ in 0..10 {
        tokio::task::yield_now().await;
        while let Ok(event) = events_rx.try_recv() {
            if matches!(event, ModelRuntimeEvent::RecoveryRecovered { .. }) {
                recovered = true;
                break;
            }
        }
        if recovered {
            break;
        }
    }

    assert!(recovered, "registry should emit RecoveryRecovered");
    assert_eq!(mutex_lock(&runtime).current_model_id(), "primary");
}

// ─── ModelFallbackReason serde/wire contract ───────────────────────────────

#[test]
fn test_model_fallback_reason_serializes_snake_case() {
    let r = ModelFallbackReason::CapacityDegrade {
        consecutive_errors: 3,
    };
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        serde_json::json!({ "capacity_degrade": { "consecutive_errors": 3 } })
    );
    let r2 = ModelFallbackReason::ProbeRecovery;
    assert_eq!(
        serde_json::to_value(&r2).unwrap(),
        serde_json::json!("probe_recovery")
    );
}

// ─── Probe: gate conditions ─────────────────────────────────────────────────

#[test]
fn test_probe_without_policy_always_skips() {
    let mut rt = with_single_fallback("primary", "fb");
    rt.advance();
    assert_eq!(rt.attempt_probe_if_due(Instant::now()), ProbeDecision::Skip);
    assert_eq!(rt.active_index(), 1);
}

#[test]
fn test_probe_while_on_primary_skips() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    assert_eq!(rt.attempt_probe_if_due(Instant::now()), ProbeDecision::Skip);
    assert_eq!(rt.active_index(), 0);
}

#[test]
fn test_probe_before_backoff_elapses_skips() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    assert_eq!(rt.attempt_probe_if_due(t0), ProbeDecision::Skip);
    assert_eq!(
        rt.attempt_probe_if_due(t0 + Duration::from_millis(500)),
        ProbeDecision::Skip,
    );
    assert_eq!(rt.active_index(), 1);
}

#[test]
fn test_probe_after_backoff_swaps_to_primary() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    let probe_time = t0 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe_time), ProbeDecision::Probe);
    assert_eq!(rt.active_index(), 0, "probe pre-swaps to primary");
    assert!(rt.probe_in_flight());
}

#[test]
fn test_probe_while_already_probing_skips() {
    // Guard: double-calling `attempt_probe_if_due` without
    // finalizing must not mutate state a second time.
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    let probe_time = t0 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe_time), ProbeDecision::Probe);
    assert_eq!(
        rt.attempt_probe_if_due(probe_time + Duration::from_secs(100)),
        ProbeDecision::Skip,
        "in-flight probe blocks further probe decisions",
    );
    assert!(rt.probe_in_flight());
    assert_eq!(rt.active_index(), 0);
}

// ─── Probe: outcomes ────────────────────────────────────────────────────────

#[test]
fn test_probe_success_clears_recovery_state() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    let probe_at = t0 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe_at), ProbeDecision::Probe);
    rt.finalize_probe(ProbeOutcome::Success, probe_at);
    assert_eq!(rt.active_index(), 0);
    assert_eq!(rt.recovery_attempts(), None, "state cleared on recovery");
    assert!(!rt.probe_in_flight());
}

#[test]
fn test_probe_failure_reverts_and_doubles_backoff() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);

    // Attempt 1 — initial backoff = 1s.
    let probe1 = t0 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe1), ProbeDecision::Probe);
    rt.finalize_probe(ProbeOutcome::Failure, probe1);
    assert_eq!(rt.active_index(), 1, "failure reverts to fallback");
    assert_eq!(rt.recovery_attempts(), Some(1));
    assert!(!rt.probe_in_flight());

    // Backoff is now 2s — 1s elapsed is still < 2s.
    assert_eq!(rt.attempt_probe_if_due(probe1), ProbeDecision::Skip);
    assert_eq!(
        rt.attempt_probe_if_due(probe1 + Duration::from_secs(1)),
        ProbeDecision::Skip,
    );

    // After 2s elapsed, probe is due again.
    let probe2 = probe1 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe2), ProbeDecision::Probe);
}

#[test]
fn test_probe_attempts_cap_halts_further_probes() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let mut clock = Instant::now();
    rt.advance_at(clock);

    // Walk through 3 failed probes — max_attempts=3.
    for _ in 0..3 {
        clock += Duration::from_secs(10); // past any backoff
        assert_eq!(rt.attempt_probe_if_due(clock), ProbeDecision::Probe);
        rt.finalize_probe(ProbeOutcome::Failure, clock);
    }
    // Fourth attempt is blocked.
    clock += Duration::from_secs(100);
    assert_eq!(rt.attempt_probe_if_due(clock), ProbeDecision::Skip);
    assert_eq!(rt.recovery_attempts(), Some(3));
}

#[test]
fn test_probe_failure_reverts_to_captured_slot_in_deep_chain() {
    // Runtime on slot 2 when probe fires — revert must restore
    // slot 2, not slot 1.
    let mut rt = ModelRuntime::new(
        stub_client("primary"),
        vec![stub_client("fb1"), stub_client("fb2")],
    )
    .with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    rt.advance_at(t0 + Duration::from_millis(100));
    assert_eq!(rt.active_index(), 2);

    let probe_at = t0 + Duration::from_secs(3);
    assert_eq!(rt.attempt_probe_if_due(probe_at), ProbeDecision::Probe);
    rt.finalize_probe(ProbeOutcome::Failure, probe_at);
    assert_eq!(
        rt.active_index(),
        2,
        "must revert to captured slot 2, not some intermediate"
    );
}

#[test]
fn test_finalize_probe_is_noop_without_in_flight_probe() {
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    // No probe started — finalize is a safe no-op.
    rt.finalize_probe(ProbeOutcome::Success, Instant::now());
    assert_eq!(rt.active_index(), 0);
    assert!(!rt.probe_in_flight());
}

// ─── Backoff ramp is monotonic across forward hops ──────────────────────────

#[test]
fn test_forward_advance_preserves_backoff_ramp() {
    // Critical contract: chain degradation MUST NOT reset the
    // probe backoff ramp. If we've been probing for a while and
    // fallback 1 then degrades, probing fallback 2 continues the
    // same backoff schedule instead of restarting at `initial_backoff`.
    let mut rt = ModelRuntime::new(
        stub_client("primary"),
        vec![stub_client("fb1"), stub_client("fb2")],
    )
    .with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);

    // One failed probe — backoff becomes 2s, attempts=1.
    let probe1 = t0 + Duration::from_secs(2);
    rt.attempt_probe_if_due(probe1);
    rt.finalize_probe(ProbeOutcome::Failure, probe1);
    assert_eq!(rt.recovery_attempts(), Some(1));

    // Forward hop to fb2 — the backoff ramp MUST preserve,
    // not reset to initial_backoff=1s.
    let hop_time = probe1 + Duration::from_secs(10);
    rt.advance_at(hop_time);
    assert_eq!(rt.active_index(), 2);
    assert_eq!(
        rt.recovery_attempts(),
        Some(1),
        "attempts preserved across forward hop",
    );

    // Immediate probe is still blocked — backoff is 2s from
    // switched_at=hop_time.
    assert_eq!(rt.attempt_probe_if_due(hop_time), ProbeDecision::Skip);
    assert_eq!(
        rt.attempt_probe_if_due(hop_time + Duration::from_secs(1)),
        ProbeDecision::Skip,
        "1s < preserved 2s backoff",
    );
    // But after 2s, due again.
    assert_eq!(
        rt.attempt_probe_if_due(hop_time + Duration::from_secs(2)),
        ProbeDecision::Probe,
    );
}

#[test]
fn test_advance_while_probe_in_flight_finalizes_as_failure() {
    // Edge case: the probe's underlying turn hit a capacity
    // error so the engine advances the chain. The runtime must
    // cleanly finalize the probe as failure before stepping.
    let mut rt = with_single_fallback("primary", "fb").with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    let probe_at = t0 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe_at), ProbeDecision::Probe);
    assert!(rt.probe_in_flight());

    // `advance_at` implicitly finalizes probe-as-failure — but
    // with one-fallback chain, the next slot is exhausted. So
    // advance reverts to fb, then tries to step forward →
    // Exhausted (already on last slot).
    //
    // For a multi-slot chain, advance would cleanly move to the
    // next tier. Single-fallback variant tests the defensive
    // finalize-then-check path.
    let outcome = rt.advance_at(probe_at);
    assert!(!rt.probe_in_flight(), "advance must finalize probe first");
    assert_eq!(outcome, AdvanceOutcome::Exhausted);
    assert_eq!(rt.recovery_attempts(), Some(1), "probe counted as failure");
}

#[test]
fn test_advance_while_probe_in_flight_deep_chain_finalizes_then_hops() {
    // Multi-slot chain: advance during probe → probe finalized
    // as failure (reverts to fb1), THEN advance to fb2.
    let mut rt = ModelRuntime::new(
        stub_client("primary"),
        vec![stub_client("fb1"), stub_client("fb2")],
    )
    .with_policy(fast_policy());
    let t0 = Instant::now();
    rt.advance_at(t0);
    assert_eq!(rt.active_index(), 1);

    let probe_at = t0 + Duration::from_secs(2);
    assert_eq!(rt.attempt_probe_if_due(probe_at), ProbeDecision::Probe);
    assert_eq!(rt.active_index(), 0, "probe pre-swap");

    let outcome = rt.advance_at(probe_at);
    assert_eq!(outcome, AdvanceOutcome::Switched("fb2".into()));
    assert_eq!(rt.active_index(), 2);
    assert!(!rt.probe_in_flight());
    assert_eq!(rt.recovery_attempts(), Some(1));
}

// ─── doubled_backoff unit test ─────────────────────────────────────────────

#[test]
fn test_doubled_backoff_saturates_at_policy_max() {
    let policy = fast_policy().recovery; // max = 4s
    assert_eq!(
        super::doubled_backoff(Duration::from_secs(1), policy),
        Duration::from_secs(2),
    );
    assert_eq!(
        super::doubled_backoff(Duration::from_secs(2), policy),
        Duration::from_secs(4),
    );
    assert_eq!(
        super::doubled_backoff(Duration::from_secs(4), policy),
        Duration::from_secs(4),
        "at cap",
    );
    assert_eq!(
        super::doubled_backoff(Duration::from_secs(100), policy),
        Duration::from_secs(4),
        "over-cap inputs still cap",
    );
}

// ── update_session_id / templated-header rebuild gate ──────────────────────

/// Build a resolved `RuntimeConfig` with one openai-compat provider whose
/// single `X-Trace` header carries `value` — mirrors `model_factory.test.rs`'s
/// `compat_with_header`, kept local so the rebuild-gate test owns its fixture.
fn runtime_with_header(value: coco_config::HeaderValue) -> Arc<RuntimeConfig> {
    use coco_config::EnvSnapshot;
    use coco_config::PartialProviderConfig;
    use coco_config::RoleSlots;
    use coco_config::RuntimeOverrides;
    use coco_config::Settings;
    use coco_config::settings::SettingsWithSource;
    use coco_types::ProviderApi;
    use coco_types::ProviderModelSelection;
    use std::collections::BTreeMap;
    use std::collections::HashMap;

    let partial = PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("LOCAL_KEY".into()),
        base_url: Some("http://localhost:8080/v1".into()),
        client_options: Some(coco_config::PartialProviderClientOptions {
            headers: Some(BTreeMap::from([("X-Trace".to_string(), value)])),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut providers = BTreeMap::new();
    providers.insert("local-router".to_string(), partial);
    let settings = Settings {
        models: coco_config::ModelSelectionSettings {
            main: Some(RoleSlots::new(ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-opus-4-7".into(),
            })),
            ..Default::default()
        },
        providers,
        ..Default::default()
    };
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let catalogs = coco_config::CatalogPaths::empty_in(tmp.path());
    let cfg = coco_config::build_runtime_config_with(
        SettingsWithSource {
            merged: settings,
            per_source: HashMap::new(),
            source_paths: HashMap::new(),
        },
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        catalogs,
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime");
    Arc::new(cfg)
}

#[test]
fn any_templated_header_detects_template_vs_literal() {
    // A `{ "template": "..." }` header → session-id swap must rebuild.
    assert!(super::any_templated_header(&runtime_with_header(
        coco_config::HeaderValue::templated("{\"session_id\":\"${SESSION_ID}\"}")
    )));
    // A plain literal header has no session-scoped dependency → no rebuild.
    assert!(!super::any_templated_header(&runtime_with_header(
        coco_config::HeaderValue::literal("static-value")
    )));
}

#[test]
fn update_session_id_swaps_header_vars_snapshot() {
    let registry = Arc::new(
        ModelRuntimeRegistry::new(
            runtime_with_header(coco_config::HeaderValue::templated("${SESSION_ID}")),
            None,
            Arc::new(HeaderVars {
                session_id: "sess-old".to_string(),
                cwd: "/work".to_string(),
                app_version: "1.2.3".to_string(),
            }),
        )
        .expect("registry"),
    );
    assert_eq!(registry.header_vars_snapshot().session_id, "sess-old");

    registry
        .update_session_id("sess-new")
        .expect("session id refresh");
    let vars = registry.header_vars_snapshot();
    assert_eq!(vars.session_id, "sess-new");
    // Non-session fields are preserved across the swap.
    assert_eq!(vars.cwd, "/work");
    assert_eq!(vars.app_version, "1.2.3");

    // Unchanged id is a no-op (and must not error).
    registry
        .update_session_id("sess-new")
        .expect("idempotent refresh");
    assert_eq!(registry.header_vars_snapshot().session_id, "sess-new");
}
