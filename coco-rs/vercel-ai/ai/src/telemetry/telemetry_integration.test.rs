use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use super::*;

/// A no-op integration that uses all defaults.
struct NoOpIntegration;

#[async_trait::async_trait]
impl TelemetryIntegration for NoOpIntegration {}

/// An integration that tracks on_error calls.
struct ErrorTrackingIntegration {
    error_seen: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl TelemetryIntegration for ErrorTrackingIntegration {
    async fn on_error(&self, _error: &(dyn std::error::Error + Send + Sync)) {
        self.error_seen.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_noop_integration_compiles() {
    let integration = NoOpIntegration;
    // Just verify the default methods exist and can be called
    // We need to construct minimal event types
    // The no-op implementations should just return ()
    let _ = &integration;
}

#[tokio::test]
async fn test_error_tracking_integration() {
    let error_seen = Arc::new(AtomicBool::new(false));
    let integration = ErrorTrackingIntegration {
        error_seen: error_seen.clone(),
    };

    let error: Box<dyn std::error::Error + Send + Sync> =
        Box::new(std::io::Error::other("test error"));
    integration.on_error(&*error).await;

    assert!(error_seen.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_execute_tool_default_passthrough() {
    let integration = NoOpIntegration;
    let result = integration.execute_tool("test_tool", async { 42 }).await;
    assert_eq!(result, 42);
}

#[tokio::test]
async fn test_execute_tool_returns_string() {
    let integration = NoOpIntegration;
    let result = integration
        .execute_tool("test_tool", async { "hello".to_string() })
        .await;
    assert_eq!(result, "hello");
}
