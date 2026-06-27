//! Telemetry integration trait.
//!
//! This module defines the `TelemetryIntegration` trait that allows
//! custom telemetry backends to receive lifecycle events from
//! generate_text, stream_text, and other functions.
//!
//! Uses the same callback event types as user callbacks.

use crate::generate_text::OnChunkEvent;
use crate::generate_text::OnFinishEvent;
use crate::generate_text::OnStartEvent;
use crate::generate_text::OnStepStartEvent;
use crate::generate_text::OnToolCallFinishEvent;
use crate::generate_text::OnToolCallStartEvent;
use crate::generate_text::StepResult;

/// Trait for custom telemetry integrations.
///
/// Implement this trait to receive lifecycle events from AI SDK functions.
/// All methods have default no-op implementations, so you only need to
/// implement the events you care about.
///
/// Uses the same event types as user callbacks.
#[async_trait::async_trait]
pub trait TelemetryIntegration: Send + Sync {
    /// Called when generation starts.
    async fn on_start(&self, _event: &OnStartEvent) {}

    /// Called when a step starts.
    async fn on_step_start(&self, _event: &OnStepStartEvent) {}

    /// Called when a tool call starts.
    async fn on_tool_call_start(&self, _event: &OnToolCallStartEvent) {}

    /// Called when a tool call finishes.
    async fn on_tool_call_finish(&self, _event: &OnToolCallFinishEvent) {}

    /// Called for each streamed chunk.
    async fn on_chunk(&self, _event: &OnChunkEvent) {}

    /// Called when a step finishes. Event IS a StepResult.
    async fn on_step_finish(&self, _event: &StepResult) {}

    /// Called when generation finishes.
    async fn on_finish(&self, _event: &OnFinishEvent) {}

    /// Called when an error occurs.
    async fn on_error(&self, _error: &(dyn std::error::Error + Send + Sync)) {}

    /// Wrap a tool execution in telemetry context.
    ///
    /// Default implementation simply calls the function directly.
    /// Override to add tracing spans, metrics, etc. around tool execution.
    async fn execute_tool<F, R>(&self, _tool_name: &str, f: F) -> R
    where
        F: std::future::Future<Output = R> + Send,
        Self: Sized,
    {
        f.await
    }
}

#[cfg(test)]
#[path = "telemetry_integration.test.rs"]
mod tests;
