//! Telemetry module.
//!
//! This module provides telemetry configuration, integration traits,
//! global registry, and dispatch utilities for monitoring AI SDK function calls.
//!
//! Telemetry integrations receive the same event types as user callbacks.

pub mod attributes;
pub mod dispatch;
mod registry;
mod telemetry_integration;
mod telemetry_settings;

pub use dispatch::build_integrations;
pub use dispatch::notify_chunk;
pub use dispatch::notify_error;
pub use dispatch::notify_finish;
pub use dispatch::notify_start;
pub use dispatch::notify_step_finish;
pub use dispatch::notify_step_start;
pub use dispatch::notify_tool_call_finish;
pub use dispatch::notify_tool_call_start;
pub use registry::clear_global_integrations;
pub use registry::get_global_integrations;
pub use registry::register_telemetry_integration;
pub use telemetry_integration::TelemetryIntegration;
pub use telemetry_settings::TelemetrySettings;
/// Preferred alias for [`TelemetrySettings`].
pub type TelemetryOptions = TelemetrySettings;
