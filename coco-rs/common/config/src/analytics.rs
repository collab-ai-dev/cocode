//! Analytics pipeline: event buffering, flush, feature flags, metrics.
//!
//! Events are queued until a sink is attached. The sink handles routing
//! to backends (Datadog, 1P logging, etc.). This module has minimal deps to
//! avoid import cycles.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use coco_types::MCP_TOOL_PREFIX;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Metadata value for analytics events.
///
/// Only primitives — never code snippets or file paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl From<bool> for MetadataValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<i64> for MetadataValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}
impl From<f64> for MetadataValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}
impl From<&str> for MetadataValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_string())
    }
}

/// Properties attached to an analytics event.
pub type EventProperties = HashMap<String, MetadataValue>;

/// An analytics event ready for dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    /// Event name (e.g. "tengu_api_success", "tengu_api_error").
    pub event_name: String,
    /// Key-value properties. No code or file paths.
    pub properties: EventProperties,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: i64,
    /// Whether this event was originally async.
    #[serde(default)]
    pub is_async: bool,
}

// ---------------------------------------------------------------------------
// Sink trait
// ---------------------------------------------------------------------------

/// Analytics backend that receives events.
pub trait AnalyticsSink: Send + Sync {
    /// Log an event synchronously (fire-and-forget to backend).
    fn log_event(&self, event_name: &str, properties: &EventProperties);
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Buffered analytics pipeline with lazy sink attachment.
///
/// Events logged before a sink is attached are queued and drained on attachment.
/// Thread-safe via `Arc<Mutex<_>>` interior.
pub struct AnalyticsPipeline {
    inner: Arc<Mutex<PipelineInner>>,
}

struct PipelineInner {
    sink: Option<Arc<dyn AnalyticsSink>>,
    queue: Vec<AnalyticsEvent>,
    /// Session-level aggregate counters.
    session_counters: HashMap<String, i64>,
    /// Feature flag cache (GrowthBook stub).
    feature_flags: HashMap<String, serde_json::Value>,
    /// Event sampling rates per event name.
    sampling_config: HashMap<String, f64>,
}

impl AnalyticsPipeline {
    /// Create a new pipeline with no sink attached.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PipelineInner {
                sink: None,
                queue: Vec::new(),
                session_counters: HashMap::new(),
                feature_flags: HashMap::new(),
                sampling_config: HashMap::new(),
            })),
        }
    }

    /// Attach a sink. Queued events are drained immediately.
    ///
    /// Idempotent: if a sink is already attached, this is a no-op.
    pub fn attach_sink(&self, sink: Arc<dyn AnalyticsSink>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.sink.is_some() {
            return;
        }
        // Drain the queue
        let queued: Vec<AnalyticsEvent> = inner.queue.drain(..).collect();
        let sink_ref = Arc::clone(&sink);
        inner.sink = Some(sink);
        // Release the lock before dispatching (avoid holding lock during I/O)
        drop(inner);

        for event in queued {
            sink_ref.log_event(&event.event_name, &event.properties);
        }
    }

    /// Log an event. If no sink is attached, the event is queued.
    pub fn log_event(&self, event_name: &str, properties: EventProperties) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Apply sampling
        if let Some(&rate) = inner.sampling_config.get(event_name)
            && !should_sample(rate)
        {
            return;
        }

        if let Some(ref sink) = inner.sink {
            let sink = Arc::clone(sink);
            drop(inner);
            sink.log_event(event_name, &properties);
        } else {
            inner.queue.push(AnalyticsEvent {
                event_name: event_name.to_string(),
                properties,
                timestamp_ms: current_ms(),
                is_async: false,
            });
        }
    }

    /// Get the number of queued (unflushed) events.
    pub fn queued_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue
            .len()
    }

    // -----------------------------------------------------------------------
    // Session-level aggregation
    // -----------------------------------------------------------------------

    /// Increment a session-level counter.
    ///
    /// Used for aggregating metrics like total API calls, total tokens, etc.
    pub fn increment_counter(&self, name: &str, delta: i64) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_counters
            .entry(name.to_string())
            .and_modify(|v| *v += delta)
            .or_insert(delta);
    }

    /// Get the current value of a session counter.
    pub fn counter_value(&self, name: &str) -> i64 {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_counters
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    /// Snapshot all session counters.
    pub fn session_counters(&self) -> HashMap<String, i64> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_counters
            .clone()
    }

    // -----------------------------------------------------------------------
    // Feature flags (GrowthBook stub)
    // -----------------------------------------------------------------------

    /// Set feature flag values (typically loaded from GrowthBook or config).
    ///
    /// Provides a simple key-value cache that can be populated from any source.
    pub fn set_feature_flags(&self, flags: HashMap<String, serde_json::Value>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .feature_flags = flags;
    }

    /// Get a feature flag value, returning the default if not set.
    pub fn get_feature_value<T: serde::de::DeserializeOwned>(&self, key: &str, default: T) -> T {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match inner.feature_flags.get(key) {
            Some(val) => serde_json::from_value(val.clone()).unwrap_or(default),
            None => default,
        }
    }

    /// Check a boolean feature gate.
    pub fn check_feature_gate(&self, gate_name: &str) -> bool {
        self.get_feature_value(gate_name, false)
    }

    // -----------------------------------------------------------------------
    // Sampling configuration
    // -----------------------------------------------------------------------

    /// Set per-event sampling rates.
    ///
    /// A rate of 1.0 means always log, 0.0 means never log, 0.1 means 10%.
    pub fn set_sampling_config(&self, config: HashMap<String, f64>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sampling_config = config;
    }

    // -----------------------------------------------------------------------
    // Reset
    // -----------------------------------------------------------------------

    /// Reset all state (for testing).
    pub fn reset(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.sink = None;
        inner.queue.clear();
        inner.session_counters.clear();
        inner.feature_flags.clear();
        inner.sampling_config.clear();
    }
}

impl Default for AnalyticsPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for AnalyticsPipeline {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ---------------------------------------------------------------------------
// Metric types (Statsig-compatible)
// ---------------------------------------------------------------------------

/// A metric event compatible with Statsig's custom event format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricEvent {
    /// Metric name.
    pub name: String,
    /// Numeric value.
    pub value: f64,
    /// Additional tags.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tags: HashMap<String, String>,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: i64,
}

/// Session-level analytics summary, emitted at session end.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionAnalytics {
    /// Total API calls in this session.
    pub api_call_count: i64,
    /// Total input tokens consumed.
    pub total_input_tokens: i64,
    /// Total output tokens generated.
    pub total_output_tokens: i64,
    /// Total cache read tokens.
    pub total_cache_read_tokens: i64,
    /// Total cache creation tokens.
    pub total_cache_creation_tokens: i64,
    /// Total duration of API calls in milliseconds.
    pub total_api_duration_ms: i64,
    /// Number of errors encountered.
    pub error_count: i64,
    /// Number of retries across all calls.
    pub retry_count: i64,
    /// Session wall-clock duration in milliseconds.
    pub session_duration_ms: i64,
    /// Number of tool calls executed.
    pub tool_call_count: i64,
    /// Estimated total cost in USD.
    pub total_cost_usd: f64,
}

impl SessionAnalytics {
    /// Cache hit rate as a percentage (0..100).
    pub fn cache_hit_rate(&self) -> f64 {
        if self.total_input_tokens == 0 {
            return 0.0;
        }
        (self.total_cache_read_tokens as f64 / self.total_input_tokens as f64) * 100.0
    }

    /// Average tokens per API call.
    pub fn avg_tokens_per_call(&self) -> f64 {
        if self.api_call_count == 0 {
            return 0.0;
        }
        (self.total_input_tokens + self.total_output_tokens) as f64 / self.api_call_count as f64
    }

    /// Convert to event properties for logging.
    pub fn to_properties(&self) -> EventProperties {
        let mut props = EventProperties::new();
        props.insert(
            "api_call_count".into(),
            MetadataValue::Int(self.api_call_count),
        );
        props.insert(
            "total_input_tokens".into(),
            MetadataValue::Int(self.total_input_tokens),
        );
        props.insert(
            "total_output_tokens".into(),
            MetadataValue::Int(self.total_output_tokens),
        );
        props.insert(
            "total_cache_read_tokens".into(),
            MetadataValue::Int(self.total_cache_read_tokens),
        );
        props.insert(
            "total_cache_creation_tokens".into(),
            MetadataValue::Int(self.total_cache_creation_tokens),
        );
        props.insert(
            "total_api_duration_ms".into(),
            MetadataValue::Int(self.total_api_duration_ms),
        );
        props.insert("error_count".into(), MetadataValue::Int(self.error_count));
        props.insert("retry_count".into(), MetadataValue::Int(self.retry_count));
        props.insert(
            "session_duration_ms".into(),
            MetadataValue::Int(self.session_duration_ms),
        );
        props.insert(
            "tool_call_count".into(),
            MetadataValue::Int(self.tool_call_count),
        );
        props.insert(
            "total_cost_usd".into(),
            MetadataValue::Float(self.total_cost_usd),
        );
        props.insert(
            "cache_hit_rate".into(),
            MetadataValue::Float(self.cache_hit_rate()),
        );
        props
    }
}

// ---------------------------------------------------------------------------
// Tool name sanitization for analytics
// ---------------------------------------------------------------------------

/// Sanitize a tool name for analytics logging to avoid PII exposure.
///
/// MCP tool names (`mcp__<server>__<tool>`) can reveal user-specific config.
/// Built-in tool names are safe to log.
pub fn sanitize_tool_name(tool_name: &str) -> &str {
    if tool_name.starts_with(MCP_TOOL_PREFIX) {
        "mcp_tool"
    } else {
        tool_name
    }
}

/// Strip `_PROTO_*` keys from properties destined for general-access storage.
pub fn strip_proto_fields(properties: &EventProperties) -> EventProperties {
    properties
        .iter()
        .filter(|(k, _)| !k.starts_with("_PROTO_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Flush interval
// ---------------------------------------------------------------------------

/// Default flush interval for buffered analytics events.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn current_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Simple deterministic sampling based on event timestamp.
fn should_sample(rate: f64) -> bool {
    if rate >= 1.0 {
        return true;
    }
    if rate <= 0.0 {
        return false;
    }
    // Use timestamp-based sampling for determinism within a millisecond
    let ts = current_ms();
    let bucket = (ts % 10000) as f64 / 10000.0;
    bucket < rate
}

#[cfg(test)]
#[path = "analytics.test.rs"]
mod tests;
