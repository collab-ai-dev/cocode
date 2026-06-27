//! Telemetry dispatch utilities.
//!
//! Provides functions to notify both user callbacks and telemetry
//! integrations for each lifecycle event. Errors are silently ignored.

use std::sync::Arc;

use super::telemetry_integration::TelemetryIntegration;

use crate::generate_text::OnChunkEvent;
use crate::generate_text::OnFinishEvent;
use crate::generate_text::OnStartEvent;
use crate::generate_text::OnStepStartEvent;
use crate::generate_text::OnToolCallFinishEvent;
use crate::generate_text::OnToolCallStartEvent;
use crate::generate_text::StepResult;

/// Notify user callback + telemetry integrations for on_start.
pub async fn notify_start(
    callback: Option<&(dyn Fn(OnStartEvent) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &OnStartEvent,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_start(event).await;
    }
}

/// Notify user callback + telemetry integrations for on_step_start.
pub async fn notify_step_start(
    callback: Option<&(dyn Fn(OnStepStartEvent) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &OnStepStartEvent,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_step_start(event).await;
    }
}

/// Notify user callback + telemetry integrations for on_tool_call_start.
pub async fn notify_tool_call_start(
    callback: Option<&(dyn Fn(OnToolCallStartEvent) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &OnToolCallStartEvent,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_tool_call_start(event).await;
    }
}

/// Notify user callback + telemetry integrations for on_tool_call_finish.
pub async fn notify_tool_call_finish(
    callback: Option<&(dyn Fn(OnToolCallFinishEvent) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &OnToolCallFinishEvent,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_tool_call_finish(event).await;
    }
}

/// Notify user callback + telemetry integrations for on_chunk.
pub async fn notify_chunk(
    callback: Option<&(dyn Fn(OnChunkEvent) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &OnChunkEvent,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_chunk(event).await;
    }
}

/// Notify user callback + telemetry integrations for on_step_finish.
pub async fn notify_step_finish(
    callback: Option<&(dyn Fn(StepResult) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &StepResult,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_step_finish(event).await;
    }
}

/// Notify user callback + telemetry integrations for on_finish.
pub async fn notify_finish(
    callback: Option<&(dyn Fn(OnFinishEvent) + Send + Sync)>,
    integrations: &[Arc<dyn TelemetryIntegration>],
    event: &OnFinishEvent,
) {
    if let Some(cb) = callback {
        cb(event.clone());
    }
    for integration in integrations {
        let _ = integration.on_finish(event).await;
    }
}

/// Notify telemetry integrations for on_error.
pub async fn notify_error(
    integrations: &[Arc<dyn TelemetryIntegration>],
    error: &(dyn std::error::Error + Send + Sync),
) {
    for integration in integrations {
        let _ = integration.on_error(error).await;
    }
}

/// Build the combined list of telemetry integrations from global registry
/// and per-call settings.
pub fn build_integrations(
    settings: Option<&super::TelemetrySettings>,
) -> Vec<Arc<dyn TelemetryIntegration>> {
    let mut integrations = super::get_global_integrations();
    if let Some(t) = settings {
        integrations.extend(t.integrations.iter().cloned());
    }
    integrations
}

#[cfg(test)]
#[path = "dispatch.test.rs"]
mod tests;
