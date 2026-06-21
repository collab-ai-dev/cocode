//! `ModelRuntime` — owns the model-call policy for a session.
//!
//! Holds an ordered chain `slots[0] = primary, slots[1..] = fallbacks`
//! and tracks which index is currently active. Public callers open calls
//! through [`ModelRuntimeRegistry`] and report completion with the returned
//! [`ModelCallHandle`]; fallback and recovery state remains inside this
//! crate.
//!
//! ## I13 (cache-state reset on provider switch)
//!
//! Provider-specific prompt-cache breakpoints are not transferable
//! across providers. When `QueryParams` construction eventually
//! consults a `CacheBreakDetector`, per-slot cache state MUST reset
//! on every active-slot transition. `on_switch_i13` is the hook
//! where that reset belongs; it is a no-op today because no
//! production caller reads per-slot cache state yet.
//!
//! ## State machine
//!
//! - `advance_at(now)`: forward hop. Slot 0→1→…→N. Preserves
//!   `RecoveryState.next_backoff` and `attempts` across the hop so
//!   the probe backoff ramp is monotonic across an entire session,
//!   not reset per tier. Only `switched_at` restarts.
//! - Background recovery: if policy configured, active != 0, backoff
//!   elapsed, attempts not exhausted → registry probes slot 0 without
//!   writing transcript or triggering tools/hooks.
//! - `finalize_probe(Success, now)`: clear recovery; switch to primary.
//! - `finalize_probe(Failure, now)`: revert to captured fallback;
//!   double backoff; increment attempts.
//!
//! Probe-in-flight state is stored on `RecoveryState` itself so the
//! engine can't forget to finalize. `current_client()` is safe to
//! call at any point and always returns the active slot.
//!
//! ## Scope
//!
//! Deliberately concrete — no trait. Public callers interact with the
//! registry/runtime call surface; the underlying `ApiClient` remains
//! private to this crate.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::MutexGuard;
use std::sync::RwLockReadGuard;
use std::sync::RwLockWriteGuard;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use coco_config::FallbackPolicy;
use coco_config::ModelInfo;
use coco_config::RecoveryProbePolicy;
use coco_config::RuntimeConfig;
use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;
use coco_types::ModelRole;
use coco_types::ModelSpec;
use coco_types::ProviderModelSelection;
use serde::Deserialize;
use serde::Serialize;

use crate::HeaderVars;
use crate::InferenceError;
use crate::LanguageModel;
use crate::ProviderClientFingerprint;
use crate::ProviderCredentialResolver;
use crate::QueryParams;
use crate::QueryResult;
use crate::RetryConfig;
use crate::StreamEvent;
use crate::client::ApiClient;
use crate::model_factory;

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
const RECOVERY_PROBE_MAX_TOKENS: i64 = 8;
const RECOVERY_PROBE_PROMPT: &str = "Reply with OK.";

/// Outcome of calling [`ModelRuntime::advance`] after a capacity error
/// in the active slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdvanceOutcome {
    /// Successfully moved to the next slot. Carries the new active
    /// model id so the caller can emit a fallback notice.
    Switched(String),
    /// All slots in the chain have been tried — the session must
    /// surface a terminal error.
    Exhausted,
}

/// Typed telemetry reason for a fallback / recovery event. Keeps the
/// wire contract closed and reviewable (vs a free-form string).
///
/// Serializes as `snake_case` strings to remain compatible with
/// downstream `ModelFallbackParams.reason: String` consumers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelFallbackReason {
    /// Forward hop triggered by a capacity error in the active slot.
    CapacityDegrade { consecutive_errors: i32 },
    /// Backward hop — half-open probe reached the primary.
    ProbeRecovery,
}

/// Recovery bookkeeping; only populated when the runtime is off primary
/// and recovery probes are enabled.
#[derive(Debug, Clone, Copy)]
struct RecoveryState {
    /// When the runtime most recently transitioned off primary OR
    /// completed a probe attempt. Seeds the backoff window.
    switched_at: Instant,
    /// Wait time before the next probe attempt. Starts at
    /// `policy.recovery.initial_backoff` and doubles on each probe
    /// failure up to `policy.recovery.max_backoff`. PRESERVED across forward
    /// `advance_at` hops so a monotonically growing backoff is not
    /// reset by chain degradation.
    next_backoff: Duration,
    /// Total probe attempts this session. Capped by
    /// `policy.recovery.max_attempts`.
    attempts: i32,
    /// When `Some(slot)`, a probe is currently in-flight; `slot` is the
    /// fallback index to keep using if the probe fails. Background probes
    /// do not pre-swap `active`, so user traffic keeps using fallback until
    /// recovery succeeds.
    probing: Option<usize>,
}

/// Outcome of a probe decision at turn entry.
///
/// `Probe` carries no payload — the slot to revert to on failure is
/// stored internally in `RecoveryState`. Engines must pair `Probe`
/// with exactly one `finalize_probe(outcome, now)` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum ProbeDecision {
    Skip,
    Probe,
}

/// Outcome the engine reports back via [`ModelRuntime::finalize_probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    /// Probe call succeeded — primary is healthy; clear recovery
    /// state and stay on slot 0.
    Success,
    /// Probe call failed. Revert to the captured pre-probe slot;
    /// double backoff; increment attempts.
    Failure,
}

/// Concrete model runtime — no trait surface.
///
/// `slots[0]` is always the primary. `slots.len()` is `1 + fallbacks.len()`;
/// `active` indexes into `slots`. Invariant: `active < slots.len()`.
pub struct ModelRuntime {
    instance_id: u64,
    slots: Vec<Arc<ApiClient>>,
    active: usize,
    generation: i64,
    policy: FallbackPolicy,
    chain_cycle: i32,
    recovery: Option<RecoveryState>,
}

/// Prebuilt model slot used by tests and embedders that already own a
/// `LanguageModel` implementation. The registry still constructs the
/// private `ApiClient`; callers never receive or name it.
#[derive(Clone)]
pub struct PrebuiltLanguageModelSlot {
    model: Arc<dyn LanguageModel>,
    retry: RetryConfig,
    fingerprint: Option<ProviderClientFingerprint>,
    model_info: Option<ModelInfo>,
    model_identity: Option<ProviderModelSelection>,
    cache_break_detector:
        Option<Arc<tokio::sync::Mutex<crate::cache_detection::CacheBreakDetector>>>,
}

impl PrebuiltLanguageModelSlot {
    pub fn new(model: Arc<dyn LanguageModel>, retry: RetryConfig) -> Self {
        Self {
            model,
            retry,
            fingerprint: None,
            model_info: None,
            model_identity: None,
            cache_break_detector: None,
        }
    }

    pub fn with_fingerprint(mut self, fingerprint: ProviderClientFingerprint) -> Self {
        self.fingerprint = Some(fingerprint);
        self
    }

    pub fn with_model_info(mut self, model_info: ModelInfo) -> Self {
        self.model_info = Some(model_info);
        self
    }

    pub fn with_model_identity(mut self, model_identity: ProviderModelSelection) -> Self {
        self.model_identity = Some(model_identity);
        self
    }

    pub fn with_cache_break_detector(
        mut self,
        detector: Arc<tokio::sync::Mutex<crate::cache_detection::CacheBreakDetector>>,
    ) -> Self {
        self.cache_break_detector = Some(detector);
        self
    }

    fn into_client(self) -> Arc<ApiClient> {
        let mut client = if let Some(fingerprint) = self.fingerprint {
            let model_identity = self
                .model_identity
                .unwrap_or_else(|| ProviderModelSelection {
                    provider: self.model.provider().to_string(),
                    model_id: self.model.model_id().to_string(),
                });
            ApiClient::new(
                self.model,
                fingerprint,
                self.model_info,
                model_identity,
                self.retry,
            )
        } else {
            let mut client = ApiClient::with_default_fingerprint(self.model, self.retry);
            if let Some(model_info) = self.model_info {
                client = client.with_model_info(model_info);
            }
            if let Some(model_identity) = self.model_identity {
                client = client.with_model_identity(model_identity);
            }
            client
        };
        let detector = self.cache_break_detector.unwrap_or_else(|| {
            Arc::new(tokio::sync::Mutex::new(
                crate::cache_detection::CacheBreakDetector::new(),
            ))
        });
        client = client.with_cache_break_detector(detector);
        Arc::new(client)
    }
}

impl ModelRuntime {
    /// Build a runtime. `fallbacks` may be empty (primary-only).
    /// This is the single constructor — prior
    /// `primary_only` / `with_fallback` convenience shims existed
    /// as `#[cfg(test)]` overlap and were removed; tests can wrap
    /// this call in a helper if needed.
    pub(crate) fn new(primary: Arc<ApiClient>, fallbacks: Vec<Arc<ApiClient>>) -> Self {
        let mut slots = Vec::with_capacity(1 + fallbacks.len());
        slots.push(primary);
        slots.extend(fallbacks);
        Self {
            instance_id: NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed),
            slots,
            active: 0,
            generation: 0,
            policy: FallbackPolicy::default(),
            chain_cycle: 1,
            recovery: None,
        }
    }

    /// Attach fallback-chain retry and primary recovery probe policy.
    pub fn with_policy(mut self, policy: FallbackPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Clone handle to the currently-active client.
    pub(crate) fn current_client(&self) -> Arc<ApiClient> {
        self.slots[self.active].clone()
    }

    /// Model id reported to tool context + telemetry.
    pub fn current_model_id(&self) -> &str {
        self.slots[self.active].model_id()
    }

    /// True when the runtime has at least one fallback slot (chain
    /// length > 1). Engine gates fallback-trigger logic on this.
    pub fn has_fallback(&self) -> bool {
        self.slots.len() > 1
    }

    /// Step forward in the chain. On success the active slot moves
    /// from N to N+1; `switched_at` refreshes but `next_backoff` and
    /// `attempts` carry forward so the probe ramp is monotonic
    /// across the session. On the last slot returns
    /// [`AdvanceOutcome::Exhausted`] without changing state.
    pub(crate) fn advance(&mut self) -> AdvanceOutcome {
        self.advance_at(Instant::now())
    }

    /// Time-injected variant of [`Self::advance`] for tests.
    pub(crate) fn advance_at(&mut self, now: Instant) -> AdvanceOutcome {
        // A forward hop while a probe is in-flight means the
        // probe's underlying turn hit a capacity error on primary.
        // Finalize the probe as a failure first so bookkeeping
        // stays consistent; the revert moves `active` to the
        // captured pre-probe slot, and THEN we check whether a
        // further advance is possible. (Without this pre-step the
        // bounds check would compare the probe's `active=0` to
        // slot count and wrongly permit advancing past the chain
        // end.)
        if self.recovery.and_then(|r| r.probing).is_some() {
            self.finalize_probe(ProbeOutcome::Failure, now);
        }
        if self.active + 1 >= self.slots.len() {
            return AdvanceOutcome::Exhausted;
        }
        self.active += 1;
        self.on_switch_i13(now);
        self.update_recovery_on_forward_advance(now);
        AdvanceOutcome::Switched(self.current_model_id().to_string())
    }

    /// Decide whether the upcoming turn should probe primary.
    ///
    /// Returns [`ProbeDecision::Probe`] when:
    /// - a recovery policy is configured, AND
    /// - the runtime is currently on a non-primary slot, AND
    /// - no probe is already in-flight, AND
    /// - the current backoff window has elapsed, AND
    /// - `attempts < policy.max_attempts`.
    ///
    /// **State mutation:** when returning `Probe`, the runtime
    /// stashes the pre-probe active slot internally and swaps
    /// `active = 0`. Engine must pair with exactly one
    /// [`Self::finalize_probe`] call before issuing the next
    /// probe-decision.
    #[cfg(test)]
    pub(crate) fn attempt_probe_if_due(&mut self, now: Instant) -> ProbeDecision {
        let policy = self.policy.recovery;
        if policy.max_attempts() == 0 {
            return ProbeDecision::Skip;
        }
        if self.active == 0 {
            return ProbeDecision::Skip;
        }
        let Some(mut state) = self.recovery else {
            return ProbeDecision::Skip;
        };
        if state.probing.is_some() {
            // Already probing — caller forgot to finalize.
            // Skipping is the safe behavior: we don't double-swap
            // and don't tick attempts. The `finalize_probe`
            // contract documents this.
            return ProbeDecision::Skip;
        }
        if state.attempts >= policy.max_attempts() {
            return ProbeDecision::Skip;
        }
        if now.duration_since(state.switched_at) < state.next_backoff {
            return ProbeDecision::Skip;
        }
        // Stash pre-probe slot and pre-swap to primary. Recovery
        // state must persist across the probe so a failure path
        // keeps `next_backoff` ready for doubling.
        state.probing = Some(self.active);
        self.recovery = Some(state);
        self.active = 0;
        self.on_switch_i13(now);
        ProbeDecision::Probe
    }

    /// Record the outcome of a probe started by
    /// [`Self::attempt_probe_if_due`]. No-op when called without an
    /// in-flight probe (defensive).
    pub(crate) fn finalize_probe(&mut self, outcome: ProbeOutcome, now: Instant) {
        let Some(state) = self.recovery else {
            return;
        };
        let Some(fallback_slot) = state.probing else {
            return;
        };
        match outcome {
            ProbeOutcome::Success => {
                // Primary is healthy — clear recovery entirely.
                self.active = 0;
                self.recovery = None;
                self.on_switch_i13(now);
            }
            ProbeOutcome::Failure => {
                // Revert to the captured fallback tier. `fallback_slot`
                // was captured before the pre-swap so it's
                // always in-range; clamp defensively anyway.
                let target = fallback_slot.min(self.slots.len() - 1).max(1);
                self.active = target;
                // Double the backoff (based on the pre-probe
                // window so the ramp is monotonic), increment
                // attempts, clear probing, refresh switched_at.
                let next_backoff = doubled_backoff(state.next_backoff, self.policy.recovery);
                self.recovery = Some(RecoveryState {
                    switched_at: now,
                    next_backoff,
                    attempts: state.attempts + 1,
                    probing: None,
                });
                self.on_switch_i13(now);
            }
        }
    }

    /// True when a probe is currently in-flight (active has been
    /// pre-swapped to primary; caller must finalize).
    #[cfg(test)]
    pub(crate) fn probe_in_flight(&self) -> bool {
        self.recovery.and_then(|r| r.probing).is_some()
    }

    fn recovery_probe_delay(&self, now: Instant) -> Option<Duration> {
        let policy = self.policy.recovery;
        if policy.max_attempts() == 0 {
            return None;
        }
        if self.active == 0 {
            return None;
        }
        let state = self.recovery?;
        if state.probing.is_some() || state.attempts >= policy.max_attempts() {
            return None;
        }
        Some(
            state
                .next_backoff
                .saturating_sub(now.duration_since(state.switched_at)),
        )
    }

    fn start_background_probe(
        &mut self,
        source: ModelRuntimeSource,
        now: Instant,
    ) -> Option<(Arc<ApiClient>, String, ModelCallHandle)> {
        if self.recovery_probe_delay(now)? != Duration::ZERO {
            return None;
        }
        let mut state = self.recovery?;
        state.probing = Some(self.active);
        self.recovery = Some(state);
        let token = ModelCallHandle {
            runtime: None,
            source,
            runtime_id: self.instance_id,
            generation: self.generation,
            slot_index: 0,
        };
        let primary = self.slots.first()?.clone();
        let model_id = primary.model_id().to_string();
        Some((primary, model_id, token))
    }

    fn finalize_background_probe(
        &mut self,
        token: &ModelCallHandle,
        source: ModelRuntimeSource,
        outcome: ProbeOutcome,
        retry_after_ms: Option<i64>,
        now: Instant,
    ) -> Vec<ModelRuntimeEvent> {
        if token.runtime_id != self.instance_id || token.generation != self.generation {
            return Vec::new();
        }
        let model_id = self.slots[0].model_id().to_string();
        match outcome {
            ProbeOutcome::Success => {
                self.finalize_probe(ProbeOutcome::Success, now);
                vec![ModelRuntimeEvent::RecoveryRecovered { source, model_id }]
            }
            ProbeOutcome::Failure => {
                self.finalize_probe(ProbeOutcome::Failure, now);
                if let (Some(retry_after_ms), Some(mut recovery)) = (retry_after_ms, self.recovery)
                {
                    let retry_after = Duration::from_millis(retry_after_ms.max(0) as u64);
                    recovery.next_backoff = recovery
                        .next_backoff
                        .max(self.policy.recovery.initial_backoff())
                        .max(retry_after);
                    self.recovery = Some(recovery);
                }
                vec![ModelRuntimeEvent::RecoveryFailed { source, model_id }]
            }
        }
    }

    /// Accessor for the recovery-state attempts counter.
    /// `None` = no recovery state (sticky or on primary).
    #[cfg(test)]
    pub(crate) fn recovery_attempts(&self) -> Option<i32> {
        self.recovery.map(|r| r.attempts)
    }

    /// Accessor for the currently-active slot index.
    #[cfg(test)]
    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    /// Accessor for total slot count (primary + fallbacks).
    #[cfg(test)]
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    // ─── Internal state management ────────────────────────────

    /// Seed or refresh recovery state after a forward hop. Preserves
    /// `next_backoff` and `attempts` across the transition so the
    /// probe ramp grows monotonically across the entire session.
    fn update_recovery_on_forward_advance(&mut self, now: Instant) {
        let policy = self.policy.recovery;
        if policy.max_attempts() == 0 {
            return;
        }
        let (next_backoff, attempts) = match self.recovery {
            Some(r) => (r.next_backoff, r.attempts),
            None => (policy.initial_backoff(), 0),
        };
        self.recovery = Some(RecoveryState {
            switched_at: now,
            next_backoff,
            attempts,
            probing: None,
        });
    }

    /// I13 cache-state reset seam. Each `ApiClient` slot owns its
    /// own `CacheBreakDetector` (installed by
    /// `model_factory::build_api_client`), so a slot transition does
    /// NOT need to wipe the previous slot's tracking state — re-
    /// activating the old slot picks up its baseline naturally. This
    /// hook is kept as the single enforcement point so future
    /// invariants (e.g. emitting a transition event, adjusting
    /// per-slot retry budgets) have one place to land.
    fn on_switch_i13(&mut self, _now: Instant) {
        self.generation += 1;
        // Per-slot detectors keep their own baseline; nothing to reset.
    }
}

/// Source used to resolve a runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelRuntimeSource {
    Role(ModelRole),
    Explicit(ProviderModelSelection),
}

/// Stable view of the currently-active model slot.
#[derive(Debug, Clone)]
pub struct ModelRuntimeSnapshot {
    pub source: ModelRuntimeSource,
    pub provider: String,
    pub provider_api: coco_types::ProviderApi,
    pub model_id: String,
    pub model_info: Option<coco_config::ModelInfo>,
    pub supports_prompt_cache: bool,
    pub supports_server_side_context_edits: bool,
    pub runtime_snapshot: coco_types::SubagentRuntimeSnapshot,
    pub active_slot: usize,
}

/// Token returned by `open_stream` so callers can report the final outcome.
#[derive(Clone)]
pub struct ModelCallHandle {
    runtime: Option<Arc<std::sync::Mutex<ModelRuntime>>>,
    source: ModelRuntimeSource,
    runtime_id: u64,
    generation: i64,
    slot_index: usize,
}

/// Communication result fed back into the runtime after a call completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCommunicationOutcome {
    Success,
    Capacity { retry_after_ms: Option<i64> },
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelRuntimeTransition {
    Noop,
    Switched(Vec<ModelRuntimeEvent>),
    RetryCurrent,
    RetryChain { backoff: Duration },
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRuntimeEvent {
    FallbackSwitched {
        source: ModelRuntimeSource,
        from_model_id: String,
        to_model_id: String,
    },
    RecoveryProbeStarted {
        source: ModelRuntimeSource,
        model_id: String,
    },
    RecoveryRecovered {
        source: ModelRuntimeSource,
        model_id: String,
    },
    RecoveryFailed {
        source: ModelRuntimeSource,
        model_id: String,
    },
    RoleRebound {
        role: ModelRole,
        model_id: String,
    },
}

pub enum ModelStreamOpenOutcome {
    Opened {
        rx: tokio::sync::mpsc::Receiver<StreamEvent>,
        token: ModelCallHandle,
        snapshot: Box<ModelRuntimeSnapshot>,
        events: Vec<ModelRuntimeEvent>,
    },
    Retry {
        events: Vec<ModelRuntimeEvent>,
    },
    Failed {
        error: InferenceError,
        events: Vec<ModelRuntimeEvent>,
    },
}

pub enum ModelRuntimeQueryOutcome {
    Success {
        result: QueryResult,
        token: ModelCallHandle,
        snapshot: Box<ModelRuntimeSnapshot>,
        events: Vec<ModelRuntimeEvent>,
    },
    Retry {
        events: Vec<ModelRuntimeEvent>,
    },
    Failed {
        error: InferenceError,
        events: Vec<ModelRuntimeEvent>,
    },
}

pub enum ModelRuntimeFeedbackOutcome {
    Retry { events: Vec<ModelRuntimeEvent> },
    Failed { events: Vec<ModelRuntimeEvent> },
}

/// Session-scoped registry for role and explicit model runtimes.
pub struct ModelRuntimeRegistry {
    runtime_config: std::sync::RwLock<Option<Arc<RuntimeConfig>>>,
    resolver: Option<Arc<dyn ProviderCredentialResolver>>,
    /// Session-scoped header-template variables (`${SESSION_ID}`, …), shared
    /// by every client this registry builds and surviving `RuntimeConfig`
    /// hot-reloads. `empty()` for prebuilt registries that don't expand
    /// templated headers. Wrapped in a `RwLock` so a `/clear` / `/resume`
    /// session-id regen can swap it in place (via [`Self::update_session_id`])
    /// and rebuild clients, instead of leaving every client baked with the old
    /// id.
    header_vars: std::sync::RwLock<Arc<HeaderVars>>,
    role_runtimes: std::sync::RwLock<HashMap<ModelRole, Arc<std::sync::Mutex<ModelRuntime>>>>,
    role_overrides: std::sync::RwLock<HashMap<ModelRole, ModelSpec>>,
    explicit_runtimes:
        std::sync::RwLock<HashMap<ProviderModelSelection, Arc<std::sync::Mutex<ModelRuntime>>>>,
    recovery_tasks: std::sync::Mutex<HashMap<ModelRuntimeSource, tokio::task::JoinHandle<()>>>,
    event_tx: tokio::sync::broadcast::Sender<ModelRuntimeEvent>,
}

#[derive(Clone)]
pub struct ModelRuntimeClient {
    registry: Arc<ModelRuntimeRegistry>,
    source: ModelRuntimeSource,
}

impl ModelRuntimeClient {
    pub fn new(registry: Arc<ModelRuntimeRegistry>, source: ModelRuntimeSource) -> Self {
        Self { registry, source }
    }

    pub fn provider(&self) -> String {
        self.registry
            .snapshot_for_source(self.source.clone())
            .map(|snapshot| snapshot.provider)
            .unwrap_or_default()
    }

    pub fn snapshot(&self) -> Result<ModelRuntimeSnapshot, InferenceError> {
        self.registry.snapshot_for_source(self.source.clone())
    }

    pub fn registry(&self) -> Arc<ModelRuntimeRegistry> {
        self.registry.clone()
    }

    pub fn source(&self) -> ModelRuntimeSource {
        self.source.clone()
    }

    pub async fn query_once(&self, params: &QueryParams) -> ModelRuntimeQueryOutcome {
        self.registry.query_once(self.source.clone(), params).await
    }

    pub async fn open_stream(&self, params: &QueryParams) -> ModelStreamOpenOutcome {
        self.registry.open_stream(self.source.clone(), params).await
    }

    pub async fn open_stream_with_rebuild<F>(
        &self,
        mut builder: F,
    ) -> Result<(tokio::sync::mpsc::Receiver<StreamEvent>, ModelCallHandle), InferenceError>
    where
        F: FnMut(&ModelRuntimeSnapshot) -> QueryParams,
    {
        loop {
            let snapshot = self.snapshot()?;
            let params = builder(&snapshot);
            match self.open_stream(&params).await {
                ModelStreamOpenOutcome::Opened { rx, token, .. } => return Ok((rx, token)),
                ModelStreamOpenOutcome::Retry { .. } => continue,
                ModelStreamOpenOutcome::Failed { error, .. } => return Err(error),
            }
        }
    }

    pub fn finish_call(
        &self,
        token: &ModelCallHandle,
        outcome: ModelCommunicationOutcome,
    ) -> Vec<ModelRuntimeEvent> {
        self.registry.finish_call(token, outcome)
    }

    pub async fn query_with_rebuild<F>(&self, mut builder: F) -> Result<QueryResult, InferenceError>
    where
        F: FnMut(&ModelRuntimeSnapshot) -> QueryParams,
    {
        loop {
            let snapshot = self.snapshot()?;
            let params = builder(&snapshot);
            match self.query_once(&params).await {
                ModelRuntimeQueryOutcome::Success { result, .. } => return Ok(result),
                ModelRuntimeQueryOutcome::Retry { .. } => continue,
                ModelRuntimeQueryOutcome::Failed { error, .. } => return Err(error),
            }
        }
    }

    pub async fn accumulated_usage(&self) -> Result<crate::UsageAccumulator, InferenceError> {
        let runtime = self.registry.runtime_for_source(self.source.clone())?;
        let client = {
            let guard = mutex_lock(&runtime);
            guard.current_client()
        };
        Ok(client.accumulated_usage().await)
    }
}

impl ModelRuntimeRegistry {
    pub fn new(
        runtime_config: Arc<RuntimeConfig>,
        resolver: Option<Arc<dyn ProviderCredentialResolver>>,
        header_vars: Arc<HeaderVars>,
    ) -> Result<Self, InferenceError> {
        let retry: RetryConfig = runtime_config.api.retry.clone().into();
        let mut role_runtimes = HashMap::new();
        for role in [
            ModelRole::Main,
            ModelRole::Plan,
            ModelRole::Fast,
            ModelRole::Explore,
            ModelRole::Review,
            ModelRole::Subagent,
            ModelRole::Memory,
            ModelRole::HookAgent,
        ] {
            if let Some(primary) = runtime_config.model_roles.get(role).cloned() {
                let runtime = build_role_runtime(
                    &runtime_config,
                    role,
                    primary,
                    retry.clone(),
                    resolver.as_ref(),
                    Some(header_vars.as_ref()),
                )?;
                role_runtimes.insert(role, Arc::new(std::sync::Mutex::new(runtime)));
            }
        }
        let (event_tx, _) = tokio::sync::broadcast::channel(128);
        Ok(Self {
            runtime_config: std::sync::RwLock::new(Some(runtime_config)),
            resolver,
            header_vars: std::sync::RwLock::new(header_vars),
            role_runtimes: std::sync::RwLock::new(role_runtimes),
            role_overrides: std::sync::RwLock::new(HashMap::new()),
            explicit_runtimes: std::sync::RwLock::new(HashMap::new()),
            recovery_tasks: std::sync::Mutex::new(HashMap::new()),
            event_tx,
        })
    }

    pub fn from_prebuilt_role_runtimes<I>(runtimes: I) -> Self
    where
        I: IntoIterator<Item = (ModelRole, ModelRuntime)>,
    {
        let (event_tx, _) = tokio::sync::broadcast::channel(128);
        Self {
            runtime_config: std::sync::RwLock::new(None),
            resolver: None,
            header_vars: std::sync::RwLock::new(Arc::new(HeaderVars::empty())),
            role_runtimes: std::sync::RwLock::new(
                runtimes
                    .into_iter()
                    .map(|(role, runtime)| (role, Arc::new(std::sync::Mutex::new(runtime))))
                    .collect(),
            ),
            role_overrides: std::sync::RwLock::new(HashMap::new()),
            explicit_runtimes: std::sync::RwLock::new(HashMap::new()),
            recovery_tasks: std::sync::Mutex::new(HashMap::new()),
            event_tx,
        }
    }

    pub fn from_prebuilt_language_model(
        role: ModelRole,
        primary: PrebuiltLanguageModelSlot,
    ) -> Self {
        Self::from_prebuilt_language_models(role, primary, Vec::new())
    }

    pub fn from_prebuilt_language_models(
        role: ModelRole,
        primary: PrebuiltLanguageModelSlot,
        fallbacks: Vec<PrebuiltLanguageModelSlot>,
    ) -> Self {
        Self::from_prebuilt_language_model_roles([(role, primary, fallbacks)])
    }

    pub fn from_prebuilt_language_model_roles<I>(runtimes: I) -> Self
    where
        I: IntoIterator<
            Item = (
                ModelRole,
                PrebuiltLanguageModelSlot,
                Vec<PrebuiltLanguageModelSlot>,
            ),
        >,
    {
        let runtimes = runtimes.into_iter().map(|(role, primary, fallbacks)| {
            let primary = primary.into_client();
            let fallbacks = fallbacks
                .into_iter()
                .map(PrebuiltLanguageModelSlot::into_client)
                .collect();
            (role, ModelRuntime::new(primary, fallbacks))
        });
        Self::from_prebuilt_role_runtimes(runtimes)
    }

    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<ModelRuntimeEvent> {
        self.event_tx.subscribe()
    }

    pub async fn open_stream(
        self: &Arc<Self>,
        source: ModelRuntimeSource,
        params: &QueryParams,
    ) -> ModelStreamOpenOutcome {
        match self.runtime_for_source(source.clone()) {
            Ok(runtime) => self.open_stream_for_runtime(runtime, source, params).await,
            Err(error) => ModelStreamOpenOutcome::Failed {
                error,
                events: Vec::new(),
            },
        }
    }

    pub async fn open_stream_for_runtime(
        self: &Arc<Self>,
        runtime: Arc<std::sync::Mutex<ModelRuntime>>,
        source: ModelRuntimeSource,
        params: &QueryParams,
    ) -> ModelStreamOpenOutcome {
        let outcome = ModelRuntime::open_stream(runtime.clone(), source, params).await;
        match &outcome {
            ModelStreamOpenOutcome::Opened { events, .. }
            | ModelStreamOpenOutcome::Retry { events }
            | ModelStreamOpenOutcome::Failed { events, .. } => {
                self.handle_runtime_events(runtime, events);
            }
        }
        outcome
    }

    pub async fn query_once(
        self: &Arc<Self>,
        source: ModelRuntimeSource,
        params: &QueryParams,
    ) -> ModelRuntimeQueryOutcome {
        let runtime = match self.runtime_for_source(source.clone()) {
            Ok(runtime) => runtime,
            Err(error) => {
                return ModelRuntimeQueryOutcome::Failed {
                    error,
                    events: Vec::new(),
                };
            }
        };
        let outcome = ModelRuntime::query_once(runtime.clone(), source, params).await;
        match &outcome {
            ModelRuntimeQueryOutcome::Success { events, .. }
            | ModelRuntimeQueryOutcome::Retry { events }
            | ModelRuntimeQueryOutcome::Failed { events, .. } => {
                self.handle_runtime_events(runtime, events);
            }
        }
        outcome
    }

    pub fn finish_call(
        self: &Arc<Self>,
        token: &ModelCallHandle,
        outcome: ModelCommunicationOutcome,
    ) -> Vec<ModelRuntimeEvent> {
        let Some(runtime) = token.runtime.clone() else {
            return Vec::new();
        };
        if !self.is_active_runtime_for_source(&runtime, &token.source) {
            return Vec::new();
        }
        let events = mutex_lock(&runtime).finish_call(token, outcome);
        self.handle_runtime_events(runtime, &events);
        events
    }

    pub async fn finish_call_for_retry(
        self: &Arc<Self>,
        token: &ModelCallHandle,
        outcome: ModelCommunicationOutcome,
    ) -> ModelRuntimeFeedbackOutcome {
        let Some(runtime) = token.runtime.clone() else {
            return ModelRuntimeFeedbackOutcome::Failed { events: Vec::new() };
        };
        if !self.is_active_runtime_for_source(&runtime, &token.source) {
            return if matches!(outcome, ModelCommunicationOutcome::Capacity { .. }) {
                ModelRuntimeFeedbackOutcome::Retry { events: Vec::new() }
            } else {
                ModelRuntimeFeedbackOutcome::Failed { events: Vec::new() }
            };
        }
        let transition = mutex_lock(&runtime).finish_call_transition(token, outcome);
        match transition {
            ModelRuntimeTransition::Switched(events) => {
                self.handle_runtime_events(runtime, &events);
                ModelRuntimeFeedbackOutcome::Retry { events }
            }
            ModelRuntimeTransition::RetryCurrent => {
                ModelRuntimeFeedbackOutcome::Retry { events: Vec::new() }
            }
            ModelRuntimeTransition::RetryChain { backoff } => {
                tracing::debug!(
                    backoff_ms = backoff.as_millis() as i64,
                    "fallback chain exhausted; retrying from primary after backoff",
                );
                if !backoff.is_zero() {
                    tokio::time::sleep(backoff).await;
                }
                ModelRuntimeFeedbackOutcome::Retry { events: Vec::new() }
            }
            ModelRuntimeTransition::Noop | ModelRuntimeTransition::Exhausted => {
                ModelRuntimeFeedbackOutcome::Failed { events: Vec::new() }
            }
        }
    }

    fn emit_events(&self, events: &[ModelRuntimeEvent]) {
        for event in events {
            let _ = self.event_tx.send(event.clone());
        }
    }

    fn handle_runtime_events(
        self: &Arc<Self>,
        runtime: Arc<std::sync::Mutex<ModelRuntime>>,
        events: &[ModelRuntimeEvent],
    ) {
        self.emit_events(events);
        for event in events {
            if let ModelRuntimeEvent::FallbackSwitched { source, .. } = event {
                self.spawn_recovery_probe(source.clone(), runtime.clone());
            }
        }
    }

    fn is_active_runtime_for_source(
        &self,
        runtime: &Arc<std::sync::Mutex<ModelRuntime>>,
        source: &ModelRuntimeSource,
    ) -> bool {
        self.runtime_for_source(source.clone())
            .is_ok_and(|active| Arc::ptr_eq(&active, runtime))
    }

    fn spawn_recovery_probe(
        self: &Arc<Self>,
        source: ModelRuntimeSource,
        runtime: Arc<std::sync::Mutex<ModelRuntime>>,
    ) {
        if !matches!(source, ModelRuntimeSource::Role(_)) {
            return;
        }
        if mutex_lock(&runtime)
            .recovery_probe_delay(Instant::now())
            .is_none()
        {
            return;
        }
        let mut recovery_tasks = mutex_lock(&self.recovery_tasks);
        if recovery_tasks.contains_key(&source) {
            return;
        }
        let registry = Arc::clone(self);
        let task_source = source.clone();
        let handle = tokio::spawn(async move {
            registry.run_recovery_probe_loop(task_source, runtime).await;
        });
        recovery_tasks.insert(source, handle);
    }

    async fn run_recovery_probe_loop(
        self: Arc<Self>,
        source: ModelRuntimeSource,
        runtime: Arc<std::sync::Mutex<ModelRuntime>>,
    ) {
        loop {
            let Some(delay) = mutex_lock(&runtime).recovery_probe_delay(Instant::now()) else {
                break;
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let Some((client, model_id, token)) =
                mutex_lock(&runtime).start_background_probe(source.clone(), Instant::now())
            else {
                continue;
            };
            self.emit_events(&[ModelRuntimeEvent::RecoveryProbeStarted {
                source: source.clone(),
                model_id,
            }]);
            let params = recovery_probe_params();
            let (outcome, retry_after_ms) = match client.query(&params).await {
                Ok(_) => (ProbeOutcome::Success, None),
                Err(error) => (ProbeOutcome::Failure, capacity_kind_from_error(&error)),
            };
            let events = mutex_lock(&runtime).finalize_background_probe(
                &token,
                source.clone(),
                outcome,
                retry_after_ms,
                Instant::now(),
            );
            self.emit_events(&events);
            if matches!(outcome, ProbeOutcome::Success) {
                break;
            }
        }
        mutex_lock(&self.recovery_tasks).remove(&source);
    }

    pub fn runtime_for_role(
        &self,
        role: ModelRole,
    ) -> Result<Arc<std::sync::Mutex<ModelRuntime>>, InferenceError> {
        if let Some(runtime) = rw_read(&self.role_runtimes).get(&role) {
            return Ok(runtime.clone());
        }
        let cfg = rw_read(&self.runtime_config).clone().ok_or_else(|| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "model_runtime",
                provider_name: role.as_str().to_string(),
                message: "role has no prebuilt runtime and no RuntimeConfig".to_string(),
            }
            .build()
        })?;
        let primary = cfg.model_roles.get(role).cloned().ok_or_else(|| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "model_runtime",
                provider_name: role.as_str().to_string(),
                message: "role has no model configured".to_string(),
            }
            .build()
        })?;
        let retry: RetryConfig = cfg.api.retry.clone().into();
        let runtime = Arc::new(std::sync::Mutex::new(build_role_runtime(
            &cfg,
            role,
            primary,
            retry,
            self.resolver.as_ref(),
            Some(self.header_vars_snapshot().as_ref()),
        )?));
        rw_write(&self.role_runtimes).insert(role, runtime.clone());
        Ok(runtime)
    }

    pub fn runtime_for_explicit(
        &self,
        selection: ProviderModelSelection,
    ) -> Result<Arc<std::sync::Mutex<ModelRuntime>>, InferenceError> {
        if let Some(runtime) = rw_read(&self.explicit_runtimes).get(&selection) {
            return Ok(runtime.clone());
        }
        let cfg = rw_read(&self.runtime_config).clone().ok_or_else(|| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "model_runtime",
                provider_name: selection.provider.clone(),
                message: "explicit runtime requested without RuntimeConfig".to_string(),
            }
            .build()
        })?;
        let spec = explicit_spec(&cfg, &selection)?;
        let retry: RetryConfig = cfg.api.retry.clone().into();
        let client = model_factory::build_api_client(
            &cfg,
            &spec,
            retry,
            self.resolver.as_ref(),
            Some(self.header_vars_snapshot().as_ref()),
        )?;
        let runtime = Arc::new(std::sync::Mutex::new(ModelRuntime::new(client, Vec::new())));
        rw_write(&self.explicit_runtimes).insert(selection, runtime.clone());
        Ok(runtime)
    }

    pub fn runtime_for_source(
        &self,
        source: ModelRuntimeSource,
    ) -> Result<Arc<std::sync::Mutex<ModelRuntime>>, InferenceError> {
        match source {
            ModelRuntimeSource::Role(role) => self.runtime_for_role(role),
            ModelRuntimeSource::Explicit(selection) => self.runtime_for_explicit(selection),
        }
    }

    pub fn snapshot_for_role(
        &self,
        role: ModelRole,
    ) -> Result<ModelRuntimeSnapshot, InferenceError> {
        let runtime = self.runtime_for_role(role)?;
        let runtime = mutex_lock(&runtime);
        Ok(runtime.snapshot(ModelRuntimeSource::Role(role)))
    }

    pub fn snapshot_for_source(
        &self,
        source: ModelRuntimeSource,
    ) -> Result<ModelRuntimeSnapshot, InferenceError> {
        let runtime = self.runtime_for_source(source.clone())?;
        let runtime = mutex_lock(&runtime);
        Ok(runtime.snapshot(source))
    }

    pub fn rebind_role_primary(
        self: &Arc<Self>,
        role: ModelRole,
        spec: ModelSpec,
    ) -> Result<Vec<ModelRuntimeEvent>, InferenceError> {
        let cfg = rw_read(&self.runtime_config).clone().ok_or_else(|| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "model_runtime",
                provider_name: role.as_str().to_string(),
                message: "role rebind requested without RuntimeConfig".to_string(),
            }
            .build()
        })?;
        let retry: RetryConfig = cfg.api.retry.clone().into();
        let runtime = build_role_runtime(
            &cfg,
            role,
            spec.clone(),
            retry,
            self.resolver.as_ref(),
            Some(self.header_vars_snapshot().as_ref()),
        )?;
        rw_write(&self.role_overrides).insert(role, spec.clone());
        if let Some(old) = mutex_lock(&self.recovery_tasks).remove(&ModelRuntimeSource::Role(role))
        {
            old.abort();
        }
        rw_write(&self.role_runtimes).insert(role, Arc::new(std::sync::Mutex::new(runtime)));
        let events = vec![ModelRuntimeEvent::RoleRebound {
            role,
            model_id: spec.model_id,
        }];
        self.emit_events(&events);
        Ok(events)
    }

    /// Snapshot the session-scoped header vars for a single client build.
    /// Cloning the `Arc` releases the lock immediately, so an in-flight
    /// `update_session_id` swap can't tear a build that has already started.
    fn header_vars_snapshot(&self) -> Arc<HeaderVars> {
        rw_read(&self.header_vars).clone()
    }

    /// Refresh the session-scoped header vars after a `/clear` or `/resume`
    /// regenerates the session id, then rebuild every client so templated
    /// headers (`${SESSION_ID}`, …) re-expand against the new value.
    ///
    /// Headers are baked into each provider client at build time, so swapping
    /// `header_vars` alone is not enough — the already-built clients keep the
    /// stale id until rebuilt. We reuse [`Self::reconcile`], but only when a
    /// configured provider actually carries a templated header; otherwise the
    /// rebuilt header maps would be byte-identical and the rebuild would just
    /// churn cache-break detectors and recovery tasks for nothing.
    ///
    /// No-op for prebuilt registries (no `RuntimeConfig`) and when the id is
    /// unchanged.
    pub fn update_session_id(self: &Arc<Self>, new_session_id: &str) -> Result<(), InferenceError> {
        {
            let mut slot = rw_write(&self.header_vars);
            if slot.session_id == new_session_id {
                return Ok(());
            }
            let prev = slot.clone();
            *slot = Arc::new(HeaderVars {
                session_id: new_session_id.to_string(),
                cwd: prev.cwd.clone(),
                app_version: prev.app_version.clone(),
            });
        }
        let cfg = rw_read(&self.runtime_config).clone();
        match cfg {
            Some(cfg) if any_templated_header(&cfg) => self.reconcile(cfg),
            _ => Ok(()),
        }
    }

    pub fn reconcile(
        self: &Arc<Self>,
        runtime_config: Arc<RuntimeConfig>,
    ) -> Result<(), InferenceError> {
        let retry: RetryConfig = runtime_config.api.retry.clone().into();
        let overrides = rw_read(&self.role_overrides).clone();
        let mut rebuilt = HashMap::new();
        for role in [
            ModelRole::Main,
            ModelRole::Plan,
            ModelRole::Fast,
            ModelRole::Explore,
            ModelRole::Review,
            ModelRole::Subagent,
            ModelRole::Memory,
            ModelRole::HookAgent,
        ] {
            let primary = overrides
                .get(&role)
                .cloned()
                .or_else(|| runtime_config.model_roles.get(role).cloned());
            if let Some(primary) = primary {
                let runtime = build_role_runtime(
                    &runtime_config,
                    role,
                    primary,
                    retry.clone(),
                    self.resolver.as_ref(),
                    Some(self.header_vars_snapshot().as_ref()),
                )?;
                rebuilt.insert(role, Arc::new(std::sync::Mutex::new(runtime)));
            }
        }
        let explicit_keys = rw_read(&self.explicit_runtimes)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut rebuilt_explicit = HashMap::new();
        for selection in explicit_keys {
            let spec = explicit_spec(&runtime_config, &selection)?;
            let client = model_factory::build_api_client(
                &runtime_config,
                &spec,
                retry.clone(),
                self.resolver.as_ref(),
                Some(self.header_vars_snapshot().as_ref()),
            )?;
            rebuilt_explicit.insert(
                selection,
                Arc::new(std::sync::Mutex::new(ModelRuntime::new(client, Vec::new()))),
            );
        }
        for (_, task) in mutex_lock(&self.recovery_tasks).drain() {
            task.abort();
        }
        *rw_write(&self.role_runtimes) = rebuilt;
        *rw_write(&self.runtime_config) = Some(runtime_config);
        *rw_write(&self.explicit_runtimes) = rebuilt_explicit;
        Ok(())
    }

    pub async fn reset_cache_break_detectors(&self) {
        let mut clients = Vec::new();
        for runtime in rw_read(&self.role_runtimes).values() {
            clients.extend(mutex_lock(runtime).clients());
        }
        for runtime in rw_read(&self.explicit_runtimes).values() {
            clients.extend(mutex_lock(runtime).clients());
        }
        for client in clients {
            client.cache_break_reset().await;
        }
    }
}

impl ModelRuntime {
    fn clients(&self) -> Vec<Arc<ApiClient>> {
        self.slots.clone()
    }

    pub fn snapshot(&self, source: ModelRuntimeSource) -> ModelRuntimeSnapshot {
        let client = self.current_client();
        ModelRuntimeSnapshot {
            source,
            provider: client.model_identity().provider.clone(),
            provider_api: client.fingerprint().api,
            model_id: client.model_identity().model_id.clone(),
            model_info: client.model_info().cloned(),
            supports_prompt_cache: client.supports_prompt_cache(),
            supports_server_side_context_edits: client.supports_server_side_context_edits(),
            runtime_snapshot: client.fingerprint().to_snapshot(),
            active_slot: self.active,
        }
    }

    pub async fn reset_active_cache_break_detector(runtime: Arc<std::sync::Mutex<Self>>) {
        let client = {
            let guard = mutex_lock(&runtime);
            guard.current_client()
        };
        client.cache_break_reset().await;
    }

    pub async fn notify_active_compaction(
        runtime: Arc<std::sync::Mutex<Self>>,
        query_source: &str,
        agent_id: Option<&str>,
    ) {
        let client = {
            let guard = mutex_lock(&runtime);
            guard.current_client()
        };
        client.notify_compaction(query_source, agent_id).await;
    }

    pub async fn notify_active_cache_deletion(
        runtime: Arc<std::sync::Mutex<Self>>,
        query_source: &str,
        agent_id: Option<&str>,
    ) {
        let client = {
            let guard = mutex_lock(&runtime);
            guard.current_client()
        };
        client.notify_cache_deletion(query_source, agent_id).await;
    }

    pub async fn cleanup_active_agent(runtime: Arc<std::sync::Mutex<Self>>, agent_id: &str) {
        let client = {
            let guard = mutex_lock(&runtime);
            guard.current_client()
        };
        client.cache_break_cleanup_agent(agent_id).await;
    }

    pub async fn open_stream(
        runtime: Arc<std::sync::Mutex<Self>>,
        source: ModelRuntimeSource,
        params: &QueryParams,
    ) -> ModelStreamOpenOutcome {
        let (client, token, snapshot) = call_context(&runtime, source.clone());
        match client.query_stream(params).await {
            Ok(rx) => ModelStreamOpenOutcome::Opened {
                rx,
                token,
                snapshot: Box::new(snapshot),
                events: Vec::new(),
            },
            Err(error) => {
                let outcome = communication_outcome_from_error(&error);
                let transition = finish_transition(&runtime, &token, outcome);
                match transition {
                    ModelRuntimeTransition::Switched(events) => {
                        ModelStreamOpenOutcome::Retry { events }
                    }
                    ModelRuntimeTransition::RetryCurrent => {
                        ModelStreamOpenOutcome::Retry { events: Vec::new() }
                    }
                    ModelRuntimeTransition::RetryChain { backoff } => {
                        tracing::debug!(
                            backoff_ms = backoff.as_millis() as i64,
                            "fallback chain exhausted while opening stream; retrying from primary after backoff",
                        );
                        if !backoff.is_zero() {
                            tokio::time::sleep(backoff).await;
                        }
                        ModelStreamOpenOutcome::Retry { events: Vec::new() }
                    }
                    ModelRuntimeTransition::Noop | ModelRuntimeTransition::Exhausted => {
                        ModelStreamOpenOutcome::Failed {
                            error,
                            events: Vec::new(),
                        }
                    }
                }
            }
        }
    }

    pub async fn query_once(
        runtime: Arc<std::sync::Mutex<Self>>,
        source: ModelRuntimeSource,
        params: &QueryParams,
    ) -> ModelRuntimeQueryOutcome {
        let (client, token, snapshot) = call_context(&runtime, source.clone());
        match client.query(params).await {
            Ok(result) => {
                let events =
                    finish_transition(&runtime, &token, ModelCommunicationOutcome::Success)
                        .into_events();
                ModelRuntimeQueryOutcome::Success {
                    result,
                    token: token.clone(),
                    snapshot: Box::new(snapshot),
                    events,
                }
            }
            Err(error) => {
                let outcome = communication_outcome_from_error(&error);
                let transition = finish_transition(&runtime, &token, outcome);
                match transition {
                    ModelRuntimeTransition::Switched(events) => {
                        ModelRuntimeQueryOutcome::Retry { events }
                    }
                    ModelRuntimeTransition::RetryCurrent => {
                        ModelRuntimeQueryOutcome::Retry { events: Vec::new() }
                    }
                    ModelRuntimeTransition::RetryChain { backoff } => {
                        tracing::debug!(
                            backoff_ms = backoff.as_millis() as i64,
                            "fallback chain exhausted during query; retrying from primary after backoff",
                        );
                        if !backoff.is_zero() {
                            tokio::time::sleep(backoff).await;
                        }
                        ModelRuntimeQueryOutcome::Retry { events: Vec::new() }
                    }
                    ModelRuntimeTransition::Noop | ModelRuntimeTransition::Exhausted => {
                        ModelRuntimeQueryOutcome::Failed {
                            error,
                            events: Vec::new(),
                        }
                    }
                }
            }
        }
    }

    pub fn finish_call(
        &mut self,
        token: &ModelCallHandle,
        outcome: ModelCommunicationOutcome,
    ) -> Vec<ModelRuntimeEvent> {
        self.finish_call_transition(token, outcome).into_events()
    }

    pub fn record_outcome(
        &mut self,
        source: ModelRuntimeSource,
        outcome: ModelCommunicationOutcome,
    ) -> Vec<ModelRuntimeEvent> {
        let token = ModelCallHandle {
            runtime: None,
            source,
            runtime_id: self.instance_id,
            generation: self.generation,
            slot_index: self.active,
        };
        self.finish_call_transition(&token, outcome).into_events()
    }

    fn finish_call_transition(
        &mut self,
        token: &ModelCallHandle,
        outcome: ModelCommunicationOutcome,
    ) -> ModelRuntimeTransition {
        if token.runtime_id != self.instance_id {
            return ModelRuntimeTransition::Noop;
        }
        if token.generation != self.generation || token.slot_index != self.active {
            if matches!(outcome, ModelCommunicationOutcome::Capacity { .. }) {
                return ModelRuntimeTransition::RetryCurrent;
            }
            return ModelRuntimeTransition::Noop;
        }
        match outcome {
            ModelCommunicationOutcome::Success => {
                self.chain_cycle = 1;
                ModelRuntimeTransition::Noop
            }
            ModelCommunicationOutcome::Failure => ModelRuntimeTransition::Noop,
            ModelCommunicationOutcome::Capacity { .. } => {
                if !self.has_fallback() {
                    return ModelRuntimeTransition::Exhausted;
                }
                let from_model_id = self.current_model_id().to_string();
                match self.advance() {
                    AdvanceOutcome::Switched(to_model_id) => {
                        ModelRuntimeTransition::Switched(vec![
                            ModelRuntimeEvent::FallbackSwitched {
                                source: token.source.clone(),
                                from_model_id,
                                to_model_id,
                            },
                        ])
                    }
                    AdvanceOutcome::Exhausted => self.retry_chain_or_exhaust(),
                }
            }
        }
    }

    fn retry_chain_or_exhaust(&mut self) -> ModelRuntimeTransition {
        let max_cycles = self.policy.exhausted_retry.max_cycles();
        if self.chain_cycle >= max_cycles {
            tracing::debug!(
                chain_cycle = self.chain_cycle,
                max_cycles,
                "fallback chain exhausted; surfacing last capacity error",
            );
            return ModelRuntimeTransition::Exhausted;
        }
        let backoff = exhausted_retry_backoff(self.chain_cycle, self.policy);
        self.chain_cycle += 1;
        self.active = 0;
        self.recovery = None;
        self.on_switch_i13(Instant::now());
        ModelRuntimeTransition::RetryChain { backoff }
    }
}

impl ModelRuntimeTransition {
    fn into_events(self) -> Vec<ModelRuntimeEvent> {
        match self {
            ModelRuntimeTransition::Switched(events) => events,
            ModelRuntimeTransition::Noop
            | ModelRuntimeTransition::RetryCurrent
            | ModelRuntimeTransition::RetryChain { .. }
            | ModelRuntimeTransition::Exhausted => Vec::new(),
        }
    }
}

fn call_context(
    runtime: &Arc<std::sync::Mutex<ModelRuntime>>,
    source: ModelRuntimeSource,
) -> (Arc<ApiClient>, ModelCallHandle, ModelRuntimeSnapshot) {
    let guard = mutex_lock(runtime);
    let snapshot = guard.snapshot(source.clone());
    let token = ModelCallHandle {
        runtime: Some(runtime.clone()),
        source,
        runtime_id: guard.instance_id,
        generation: guard.generation,
        slot_index: guard.active,
    };
    (guard.current_client(), token, snapshot)
}

fn finish_transition(
    runtime: &std::sync::Mutex<ModelRuntime>,
    token: &ModelCallHandle,
    outcome: ModelCommunicationOutcome,
) -> ModelRuntimeTransition {
    mutex_lock(runtime).finish_call_transition(token, outcome)
}

fn build_role_runtime(
    runtime_config: &RuntimeConfig,
    role: ModelRole,
    primary: ModelSpec,
    retry: RetryConfig,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    header_vars: Option<&HeaderVars>,
) -> Result<ModelRuntime, InferenceError> {
    let primary = model_factory::build_api_client(
        runtime_config,
        &primary,
        retry.clone(),
        resolver,
        header_vars,
    )?;
    let fallbacks = runtime_config
        .model_roles
        .fallbacks(role)
        .iter()
        .map(|spec| {
            model_factory::build_api_client(
                runtime_config,
                spec,
                retry.clone(),
                resolver,
                header_vars,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let runtime = ModelRuntime::new(primary, fallbacks);
    Ok(match runtime_config.model_roles.policy(role) {
        Some(policy) => runtime.with_policy(policy),
        None => runtime,
    })
}

fn explicit_spec(
    runtime_config: &RuntimeConfig,
    selection: &ProviderModelSelection,
) -> Result<ModelSpec, InferenceError> {
    let provider_cfg = runtime_config
        .providers
        .get(&selection.provider)
        .ok_or_else(|| {
            crate::errors::UnknownProviderSnafu {
                provider: selection.provider.clone(),
            }
            .build()
        })?;
    let display_name = runtime_config
        .model_registry
        .resolve(&selection.provider, &selection.model_id)
        .and_then(|m| m.info.display_name.clone())
        .unwrap_or_else(|| selection.model_id.clone());
    Ok(ModelSpec {
        provider: selection.provider.clone(),
        api: provider_cfg.api,
        model_id: selection.model_id.clone(),
        display_name,
    })
}

fn recovery_probe_params() -> QueryParams {
    QueryParams {
        prompt: vec![LlmMessage::User {
            content: vec![UserContentPart::text(RECOVERY_PROBE_PROMPT)],
            provider_options: None,
        }],
        max_tokens: Some(RECOVERY_PROBE_MAX_TOKENS),
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: None,
        agent_id: None,
        time_since_last_assistant_ms: None,
        cache: None,
        agentic: false,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    }
}

fn capacity_kind_from_error(error: &InferenceError) -> Option<i64> {
    match error {
        InferenceError::Overloaded { retry_after_ms, .. }
        | InferenceError::RateLimited { retry_after_ms, .. } => *retry_after_ms,
        _ => match InferenceError::classify_stream_message(&error.to_string()) {
            Some(InferenceError::Overloaded { retry_after_ms, .. })
            | Some(InferenceError::RateLimited { retry_after_ms, .. }) => retry_after_ms,
            _ => None,
        },
    }
}

fn communication_outcome_from_error(error: &InferenceError) -> ModelCommunicationOutcome {
    match error {
        InferenceError::Overloaded { retry_after_ms, .. }
        | InferenceError::RateLimited { retry_after_ms, .. } => {
            ModelCommunicationOutcome::Capacity {
                retry_after_ms: *retry_after_ms,
            }
        }
        _ => match InferenceError::classify_stream_message(&error.to_string()) {
            Some(
                InferenceError::Overloaded { retry_after_ms, .. }
                | InferenceError::RateLimited { retry_after_ms, .. },
            ) => ModelCommunicationOutcome::Capacity { retry_after_ms },
            _ => ModelCommunicationOutcome::Failure,
        },
    }
}

/// Whether any configured provider carries a templated header value
/// (`{ "template": "..." }`). Literal-only header maps don't depend on the
/// session-scoped vars, so a session-id swap need not rebuild their clients.
fn any_templated_header(cfg: &RuntimeConfig) -> bool {
    cfg.providers.values().any(|p| {
        p.client_options
            .headers
            .values()
            .any(coco_config::HeaderValue::is_templated)
    })
}

fn rw_read<T>(lock: &std::sync::RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn rw_write<T>(lock: &std::sync::RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mutex_lock<T>(lock: &std::sync::Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn exhausted_retry_backoff(completed_cycle: i32, policy: FallbackPolicy) -> Duration {
    let retry = policy.exhausted_retry;
    let mut backoff = retry.initial_backoff();
    for _ in 1..completed_cycle.max(1) {
        backoff = backoff.saturating_mul(2).min(retry.max_backoff());
    }
    backoff.min(retry.max_backoff())
}

/// Double the current backoff, clamping to `policy.max_backoff`.
/// Free function so tests can exercise the ramp in isolation.
fn doubled_backoff(current: Duration, policy: RecoveryProbePolicy) -> Duration {
    let max = policy.max_backoff();
    let doubled = current.saturating_mul(2);
    if doubled > max { max } else { doubled }
}

#[cfg(test)]
#[path = "model_runtime.test.rs"]
mod tests;
