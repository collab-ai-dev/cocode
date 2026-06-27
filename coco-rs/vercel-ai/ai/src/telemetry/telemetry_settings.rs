//! Telemetry settings configuration.
//!
//! This module provides the `TelemetrySettings` type for configuring telemetry
//! in AI SDK function calls.

use std::sync::Arc;

use vercel_ai_provider::json_value::JSONValue;

use super::telemetry_integration::TelemetryIntegration;

/// Telemetry configuration.
///
/// Configures telemetry behavior and allows registering custom
/// telemetry integrations that receive lifecycle events.
#[derive(Clone, Default)]
pub struct TelemetrySettings {
    /// Enable or disable telemetry. Disabled by default while experimental.
    pub is_enabled: Option<bool>,

    /// Enable or disable input recording. Enabled by default.
    ///
    /// You might want to disable input recording to avoid recording sensitive
    /// information, to reduce data transfers, or to increase performance.
    pub record_inputs: Option<bool>,

    /// Enable or disable output recording. Enabled by default.
    ///
    /// You might want to disable output recording to avoid recording sensitive
    /// information, to reduce data transfers, or to increase performance.
    pub record_outputs: Option<bool>,

    /// Identifier for this function. Used to group telemetry data by function.
    pub function_id: Option<String>,

    /// Additional information to include in the telemetry data.
    pub metadata: Option<JSONValue>,

    /// Custom telemetry integrations that receive lifecycle events.
    pub integrations: Vec<Arc<dyn TelemetryIntegration>>,
}

impl std::fmt::Debug for TelemetrySettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetrySettings")
            .field("is_enabled", &self.is_enabled)
            .field("record_inputs", &self.record_inputs)
            .field("record_outputs", &self.record_outputs)
            .field("function_id", &self.function_id)
            .field("metadata", &self.metadata)
            .field("integrations_count", &self.integrations.len())
            .finish()
    }
}

impl TelemetrySettings {
    /// Create new telemetry settings with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable telemetry.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.is_enabled = Some(enabled);
        self
    }

    /// Set whether to record inputs.
    pub fn with_record_inputs(mut self, record: bool) -> Self {
        self.record_inputs = Some(record);
        self
    }

    /// Set whether to record outputs.
    pub fn with_record_outputs(mut self, record: bool) -> Self {
        self.record_outputs = Some(record);
        self
    }

    /// Set the function ID.
    pub fn with_function_id(mut self, id: impl Into<String>) -> Self {
        self.function_id = Some(id.into());
        self
    }

    /// Set additional metadata.
    pub fn with_metadata(mut self, metadata: JSONValue) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Add a telemetry integration.
    pub fn with_integration(mut self, integration: Arc<dyn TelemetryIntegration>) -> Self {
        self.integrations.push(integration);
        self
    }

    /// Check if telemetry is enabled.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled.unwrap_or(false)
    }

    /// Check if input recording is enabled.
    pub fn should_record_inputs(&self) -> bool {
        self.record_inputs.unwrap_or(true)
    }

    /// Check if output recording is enabled.
    pub fn should_record_outputs(&self) -> bool {
        self.record_outputs.unwrap_or(true)
    }
}

#[cfg(test)]
#[path = "telemetry_settings.test.rs"]
mod tests;
