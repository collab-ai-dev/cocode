use coco_model_card::Pricing;
use coco_types::ProviderModelSelection;
use coco_types::SessionModelUsageEntry;
use coco_types::SessionUsageSnapshot;
use coco_types::SessionUsageTotals;
use coco_types::TokenUsage;
use std::collections::HashMap;

pub const SESSION_USAGE_SNAPSHOT_VERSION: i32 = 1;

/// Tracks cost and token usage per provider/model across a session.
#[derive(Debug, Clone, Default)]
pub struct CostTracker {
    per_model: HashMap<ProviderModelSelection, SessionModelUsageEntry>,
    pub total_api_calls: i64,
    pub total_duration_ms: i64,
}

impl CostTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record usage from a single API call.
    pub fn record_usage(
        &mut self,
        provider: &str,
        model_id: &str,
        usage: TokenUsage,
        duration_ms: i64,
    ) {
        let costs = usage_cost_usd(provider, model_id, &usage);
        let key = ProviderModelSelection {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        };
        let entry = self
            .per_model
            .entry(key.clone())
            .or_insert_with(|| SessionModelUsageEntry {
                provider: key.provider.clone(),
                model_id: key.model_id.clone(),
                priced: true,
                ..Default::default()
            });
        entry.input_tokens = entry.input_tokens.saturating_add(usage.input_tokens.total);
        entry.output_tokens = entry
            .output_tokens
            .saturating_add(usage.output_tokens.total);
        entry.cache_read_input_tokens = entry
            .cache_read_input_tokens
            .saturating_add(usage.input_tokens.cache_read);
        entry.cache_creation_input_tokens = entry
            .cache_creation_input_tokens
            .saturating_add(usage.input_tokens.cache_write);
        entry.input_cost_usd += costs.input_cost_usd;
        entry.output_cost_usd += costs.output_cost_usd;
        entry.cache_read_cost_usd += costs.cache_read_cost_usd;
        entry.cache_creation_cost_usd += costs.cache_creation_cost_usd;
        entry.total_cost_usd += costs.total_cost_usd;
        entry.request_count = entry.request_count.saturating_add(1);
        if !costs.priced {
            entry.unpriced_request_count = entry.unpriced_request_count.saturating_add(1);
            entry.unpriced_input_tokens = entry
                .unpriced_input_tokens
                .saturating_add(usage.input_tokens.total);
            entry.unpriced_output_tokens = entry
                .unpriced_output_tokens
                .saturating_add(usage.output_tokens.total);
        }
        entry.priced = entry.unpriced_request_count == 0;
        self.total_api_calls = self.total_api_calls.saturating_add(1);
        self.total_duration_ms = self.total_duration_ms.saturating_add(duration_ms);
    }

    /// Total cost across all models.
    pub fn total_cost_usd(&self) -> f64 {
        self.per_model.values().map(|u| u.total_cost_usd).sum()
    }

    /// Total input tokens across all models.
    pub fn total_input_tokens(&self) -> i64 {
        self.per_model.values().map(|u| u.input_tokens).sum()
    }

    /// Total output tokens across all models.
    pub fn total_output_tokens(&self) -> i64 {
        self.per_model.values().map(|u| u.output_tokens).sum()
    }

    pub fn model_entries(
        &self,
    ) -> impl Iterator<Item = (&ProviderModelSelection, &SessionModelUsageEntry)> {
        self.per_model.iter()
    }

    pub fn snapshot(&self, session_id: impl Into<String>) -> SessionUsageSnapshot {
        self.snapshot_at(session_id, timestamp_now_ms())
    }

    pub fn snapshot_at(
        &self,
        session_id: impl Into<String>,
        updated_at_ms: i64,
    ) -> SessionUsageSnapshot {
        let mut models: Vec<_> = self.per_model.values().cloned().collect();
        models.sort_by(|a, b| {
            a.provider
                .cmp(&b.provider)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });

        let mut totals = SessionUsageTotals::default();
        for entry in &models {
            totals.input_tokens = totals.input_tokens.saturating_add(entry.input_tokens);
            totals.output_tokens = totals.output_tokens.saturating_add(entry.output_tokens);
            totals.cache_read_input_tokens = totals
                .cache_read_input_tokens
                .saturating_add(entry.cache_read_input_tokens);
            totals.cache_creation_input_tokens = totals
                .cache_creation_input_tokens
                .saturating_add(entry.cache_creation_input_tokens);
            totals.input_cost_usd += entry.input_cost_usd;
            totals.output_cost_usd += entry.output_cost_usd;
            totals.cache_read_cost_usd += entry.cache_read_cost_usd;
            totals.cache_creation_cost_usd += entry.cache_creation_cost_usd;
            totals.total_cost_usd += entry.total_cost_usd;
            totals.request_count = totals.request_count.saturating_add(entry.request_count);
            totals.web_search_requests = totals
                .web_search_requests
                .saturating_add(entry.web_search_requests);
            totals.unpriced_request_count = totals
                .unpriced_request_count
                .saturating_add(entry.unpriced_request_count);
            totals.unpriced_input_tokens = totals
                .unpriced_input_tokens
                .saturating_add(entry.unpriced_input_tokens);
            totals.unpriced_output_tokens = totals
                .unpriced_output_tokens
                .saturating_add(entry.unpriced_output_tokens);
        }

        let unpriced_models = models
            .iter()
            .filter(|entry| entry.unpriced_request_count > 0)
            .map(|entry| ProviderModelSelection {
                provider: entry.provider.clone(),
                model_id: entry.model_id.clone(),
            })
            .collect();

        SessionUsageSnapshot {
            version: SESSION_USAGE_SNAPSHOT_VERSION,
            session_id: session_id.into(),
            updated_at_ms,
            totals,
            models,
            unpriced_models,
        }
    }

    pub fn from_snapshot(snapshot: SessionUsageSnapshot) -> Self {
        let mut tracker = Self::new();
        for mut entry in snapshot.models {
            if !entry.priced && entry.unpriced_request_count == 0 {
                entry.unpriced_request_count = entry.request_count;
                entry.unpriced_input_tokens = entry.input_tokens;
                entry.unpriced_output_tokens = entry.output_tokens;
            }
            entry.priced = entry.unpriced_request_count == 0;
            tracker.total_api_calls = tracker.total_api_calls.saturating_add(entry.request_count);
            tracker.per_model.insert(
                ProviderModelSelection {
                    provider: entry.provider.clone(),
                    model_id: entry.model_id.clone(),
                },
                entry,
            );
        }
        tracker
    }
}

/// Per-model pricing data (USD per million tokens).
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

impl From<Pricing> for ModelPricing {
    fn from(value: Pricing) -> Self {
        Self {
            input_per_mtok: value.input_per_million_usd,
            output_per_mtok: value.output_per_million_usd,
            cache_write_per_mtok: value
                .cache_write_per_million_usd
                .unwrap_or(value.input_per_million_usd),
            cache_read_per_mtok: value
                .cache_read_per_million_usd
                .unwrap_or(value.input_per_million_usd),
        }
    }
}

/// Get pricing for a model by provider and model id.
pub fn get_model_pricing(provider: Option<&str>, model_id: &str) -> Option<ModelPricing> {
    coco_model_card::pricing(provider, model_id).map(ModelPricing::from)
}

/// Calculate USD cost from token counts and model.
pub fn calculate_cost_usd(provider: Option<&str>, model_id: &str, usage: &TokenUsage) -> f64 {
    usage_cost_usd(provider.unwrap_or_default(), model_id, usage).total_cost_usd
}

/// Format cost as a human-readable string.
///
/// Costs above $0.50 render with 2 decimals, otherwise 4 decimals.
/// The `> 0.5` boundary is strict, so $0.50 itself takes the 4-decimal branch.
pub fn format_cost(cost_usd: f64) -> String {
    if cost_usd > 0.5 {
        format!("${cost_usd:.2}")
    } else {
        format!("${cost_usd:.4}")
    }
}

/// Group an integer with thousands separators (e.g. `1234567` → `1,234,567`).
fn group_thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut out: String = grouped.chars().rev().collect();
    if neg {
        out.insert(0, '-');
    }
    out
}

/// Render a per-model session cost breakdown from a live
/// [`coco_types::SessionUsageSnapshot`].
///
/// This is the source `/cost` should display: it is multi-provider (pricing
/// already resolved via `coco_model_card` when the snapshot was accumulated by
/// [`CostTracker`]) and flags unpriced models, rather than re-deriving cost
/// from a stale session file with hardcoded single-provider pricing.
pub fn format_session_cost(snapshot: &coco_types::SessionUsageSnapshot) -> String {
    use std::fmt::Write as _;

    let mut out = String::from("## Session Cost\n\n");
    if snapshot.models.is_empty() {
        out.push_str("No API usage recorded yet.\n\n");
        out.push_str("Cost tracking begins when the first API request is made.");
        return out;
    }

    for m in &snapshot.models {
        let _ = writeln!(out, "### {} / {}\n", m.provider, m.model_id);
        let _ = writeln!(
            out,
            "  Input tokens:       {:>12}",
            group_thousands(m.input_tokens)
        );
        let _ = writeln!(
            out,
            "  Output tokens:      {:>12}",
            group_thousands(m.output_tokens)
        );
        let _ = writeln!(
            out,
            "  Cache read tokens:  {:>12}",
            group_thousands(m.cache_read_input_tokens)
        );
        let _ = writeln!(
            out,
            "  Cache write tokens: {:>12}",
            group_thousands(m.cache_creation_input_tokens)
        );
        let _ = writeln!(out, "  API requests:       {:>12}", m.request_count);
        if m.priced {
            let _ = writeln!(
                out,
                "  Cost:               {}\n",
                format_cost(m.total_cost_usd)
            );
        } else {
            out.push_str("  Cost:               (unpriced model — no pricing data)\n\n");
        }
    }

    let t = &snapshot.totals;
    out.push_str("### Total\n\n");
    let _ = writeln!(out, "  Input tokens:  {}", group_thousands(t.input_tokens));
    let _ = writeln!(out, "  Output tokens: {}", group_thousands(t.output_tokens));
    let _ = writeln!(out, "  API requests:  {}", t.request_count);
    let _ = write!(out, "  **Total cost:  {}**", format_cost(t.total_cost_usd));
    if !snapshot.unpriced_models.is_empty() {
        let _ = write!(
            out,
            "\n\n_Note: {} model(s) had no pricing data and are excluded from the cost total._",
            snapshot.unpriced_models.len()
        );
    }
    out
}

#[derive(Debug, Clone, Copy, Default)]
struct UsageCost {
    input_cost_usd: f64,
    output_cost_usd: f64,
    cache_read_cost_usd: f64,
    cache_creation_cost_usd: f64,
    total_cost_usd: f64,
    priced: bool,
}

fn usage_cost_usd(provider: &str, model_id: &str, usage: &TokenUsage) -> UsageCost {
    let Some(pricing) = get_model_pricing(non_empty_provider(provider), model_id) else {
        return UsageCost::default();
    };
    let uncached_input = uncached_input_tokens(usage);
    let input_cost_usd = token_cost(uncached_input, pricing.input_per_mtok);
    let output_cost_usd = token_cost(usage.output_tokens.total, pricing.output_per_mtok);
    let cache_read_cost_usd =
        token_cost(usage.input_tokens.cache_read, pricing.cache_read_per_mtok);
    let cache_creation_cost_usd =
        token_cost(usage.input_tokens.cache_write, pricing.cache_write_per_mtok);
    UsageCost {
        input_cost_usd,
        output_cost_usd,
        cache_read_cost_usd,
        cache_creation_cost_usd,
        total_cost_usd: input_cost_usd
            + output_cost_usd
            + cache_read_cost_usd
            + cache_creation_cost_usd,
        priced: true,
    }
}

fn uncached_input_tokens(usage: &TokenUsage) -> i64 {
    if usage.input_tokens.no_cache > 0 {
        usage.input_tokens.no_cache
    } else {
        usage
            .input_tokens
            .total
            .saturating_sub(usage.input_tokens.cache_read)
            .saturating_sub(usage.input_tokens.cache_write)
    }
}

fn token_cost(tokens: i64, per_million: f64) -> f64 {
    tokens as f64 * per_million / 1_000_000.0
}

fn non_empty_provider(provider: &str) -> Option<&str> {
    if provider.is_empty() {
        None
    } else {
        Some(provider)
    }
}

fn timestamp_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "cost.test.rs"]
mod tests;
