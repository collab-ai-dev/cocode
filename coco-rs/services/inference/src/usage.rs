use coco_types::ProviderModelSelection;
use coco_types::TokenUsage;
use std::collections::HashMap;

/// Accumulates token usage across multiple API calls.
///
/// `per_model` is keyed by [`ProviderModelSelection`] so cost analytics
/// distinguish `openai/gpt-5` from `openrouter/openai/gpt-5` etc.
/// **Wire format note**: when serialized via serde, the map becomes a
/// `Vec<(K, V)>` (JSON `[[ {provider, model_id}, {usage} ]]`) rather
/// than an object — `ProviderModelSelection` is a struct, not a string.
/// SDK consumers that previously read `accumulated_usage.per_model[id]`
/// must adapt; the refactor intentionally drops the lossy string
/// representation.
#[derive(Debug, Clone, Default)]
pub struct UsageAccumulator {
    /// Total usage across all calls.
    pub total: TokenUsage,
    /// Per-(provider, model_id) usage breakdown.
    pub per_model: HashMap<ProviderModelSelection, TokenUsage>,
    /// Number of API calls made.
    pub call_count: i64,
}

impl UsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record usage from a single API call.
    ///
    /// Caller passes the (provider, model_id) identity that produced the
    /// usage. `ApiClient` caches its own `provider_model` once at
    /// construction so the per-call cost here is one `clone()` of a
    /// two-`String` struct.
    pub fn record(&mut self, model: &ProviderModelSelection, usage: TokenUsage) {
        self.total += usage;
        self.per_model
            .entry(model.clone())
            .and_modify(|u| *u += usage)
            .or_insert(usage);
        self.call_count += 1;
    }
}
