//! Telemetry and analytics event system.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

/// An analytics event to log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    pub event_name: String,
    pub properties: HashMap<String, serde_json::Value>,
    pub timestamp_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Analytics metadata for safe logging (no code or file paths).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalyticsMetadata {
    #[serde(flatten)]
    pub fields: HashMap<String, serde_json::Value>,
}

/// Telemetry configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub sample_rate: Option<f64>,
}

/// Event types for structured logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    ToolUse,
    Permission,
    Compact,
    Session,
    Error,
    Model,
    Budget,
    Hook,
    Plugin,
    Mcp,
}

/// Log an analytics event (in-memory buffer, flushed periodically).
pub struct AnalyticsLogger {
    events: Vec<AnalyticsEvent>,
    config: TelemetryConfig,
    session_id: String,
}

impl AnalyticsLogger {
    pub fn new(config: TelemetryConfig, session_id: String) -> Self {
        Self {
            events: Vec::new(),
            config,
            session_id,
        }
    }

    /// Log an event.
    pub fn log_event(&mut self, name: &str, properties: HashMap<String, serde_json::Value>) {
        if !self.config.enabled {
            return;
        }
        self.events.push(AnalyticsEvent {
            event_name: name.to_string(),
            properties,
            timestamp_ms: current_ms(),
            session_id: Some(self.session_id.clone()),
        });
    }

    /// Log a tool use event.
    pub fn log_tool_use(&mut self, tool_name: &str, duration_ms: i64, is_error: bool) {
        let mut props = HashMap::new();
        props.insert("tool_name".into(), serde_json::json!(tool_name));
        props.insert("duration_ms".into(), serde_json::json!(duration_ms));
        props.insert("is_error".into(), serde_json::json!(is_error));
        self.log_event("tool_use", props);
    }

    /// Log a permission decision.
    pub fn log_permission(&mut self, tool_name: &str, decision: &str, reason: &str) {
        let mut props = HashMap::new();
        props.insert("tool_name".into(), serde_json::json!(tool_name));
        props.insert("decision".into(), serde_json::json!(decision));
        props.insert("reason".into(), serde_json::json!(reason));
        self.log_event("permission_decision", props);
    }

    /// Get all buffered events (for flush).
    pub fn drain_events(&mut self) -> Vec<AnalyticsEvent> {
        std::mem::take(&mut self.events)
    }

    /// Number of buffered events.
    pub fn pending_count(&self) -> usize {
        self.events.len()
    }
}

fn current_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "telemetry.test.rs"]
mod tests;
