mod client;
mod config;
mod error;
pub(crate) mod timer;
pub(crate) mod validation;

pub use crate::metrics::client::MetricsClient;
pub use crate::metrics::config::MetricsConfig;
pub use crate::metrics::config::MetricsExporter;
pub use crate::metrics::error::MetricsError;
pub use crate::metrics::error::Result;
use std::sync::OnceLock;

static GLOBAL_METRICS: OnceLock<MetricsClient> = OnceLock::new();

pub(crate) fn install_global(metrics: MetricsClient) {
    let _ = GLOBAL_METRICS.set(metrics);
}

pub(crate) fn global() -> Option<MetricsClient> {
    GLOBAL_METRICS.get().cloned()
}

/// Record a counter increment via the global metrics handle, if installed.
///
/// No-op when no exporter has been attached (common case in dev, tests, and
/// any run without `--otel-exporter`). Errors from the underlying client
/// are logged at `debug` and swallowed — a broken metrics subsystem must
/// never take down the caller.
///
/// This is the idiomatic entry point for downstream crates (e.g.
/// `coco-query`, `coco-cli`) that want to record metrics without taking a
/// hard dependency on owning an `OtelManager`.
pub fn record_counter(name: &str, inc: i64, tags: &[(&str, &str)]) {
    let Some(client) = global() else {
        return;
    };
    if let Err(e) = client.counter(name, inc, tags) {
        tracing::debug!(metric = name, error = %e, "metric counter record failed");
    }
}

/// Record a histogram sample via the global metrics handle, if installed.
///
/// See [`record_counter`] for error/no-op semantics.
pub fn record_histogram(name: &str, value: i64, tags: &[(&str, &str)]) {
    let Some(client) = global() else {
        return;
    };
    if let Err(e) = client.histogram(name, value, tags) {
        tracing::debug!(metric = name, error = %e, "metric histogram record failed");
    }
}
