//! Global telemetry integration registry.
//!
//! Allows registering telemetry integrations globally so they receive
//! events from all generate_text/stream_text calls.

use std::sync::Arc;
use std::sync::Mutex;

use once_cell::sync::Lazy;

use super::telemetry_integration::TelemetryIntegration;

static GLOBAL_INTEGRATIONS: Lazy<Mutex<Vec<Arc<dyn TelemetryIntegration>>>> =
    Lazy::new(|| Mutex::new(Vec::new()));

/// Register a telemetry integration globally.
///
/// All registered integrations will receive lifecycle events from
/// generate_text, stream_text, and other AI SDK functions.
#[allow(clippy::expect_used)]
pub fn register_telemetry_integration(integration: Arc<dyn TelemetryIntegration>) {
    GLOBAL_INTEGRATIONS
        .lock()
        .expect("lock poisoned")
        .push(integration);
}

/// Get all globally registered telemetry integrations.
#[allow(clippy::expect_used)]
pub fn get_global_integrations() -> Vec<Arc<dyn TelemetryIntegration>> {
    GLOBAL_INTEGRATIONS.lock().expect("lock poisoned").clone()
}

/// Clear all globally registered telemetry integrations.
#[allow(clippy::expect_used)]
pub fn clear_global_integrations() {
    GLOBAL_INTEGRATIONS.lock().expect("lock poisoned").clear();
}

#[cfg(test)]
#[path = "registry.test.rs"]
mod tests;
