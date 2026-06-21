use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use super::*;
use crate::generate_text::CallbackModelInfo;
use crate::generate_text::OnStartEvent;
use crate::generate_text::StepResult;

struct CountingIntegration {
    start_count: AtomicU32,
    finish_count: AtomicU32,
}

impl CountingIntegration {
    fn new() -> Self {
        Self {
            start_count: AtomicU32::new(0),
            finish_count: AtomicU32::new(0),
        }
    }
}

#[async_trait::async_trait]
impl crate::telemetry::TelemetryIntegration for CountingIntegration {
    async fn on_start(&self, _event: &OnStartEvent) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_finish(&self, _event: &crate::generate_text::OnFinishEvent) {
        self.finish_count.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_notify_start_with_callback_and_integration() {
    let callback_called = Arc::new(AtomicU32::new(0));
    let callback_called_clone = callback_called.clone();
    let callback = move |_: OnStartEvent| {
        callback_called_clone.fetch_add(1, Ordering::SeqCst);
    };

    let integration = Arc::new(CountingIntegration::new());
    let integration_ref = integration.clone();
    let integrations: Vec<Arc<dyn crate::telemetry::TelemetryIntegration>> = vec![integration];

    let model = CallbackModelInfo::new("test", "test-model");
    let event = OnStartEvent::new("call-1", model);

    notify_start(Some(&callback), &integrations, &event).await;

    assert_eq!(callback_called.load(Ordering::SeqCst), 1);
    assert_eq!(integration_ref.start_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_notify_start_without_callback() {
    let integration = Arc::new(CountingIntegration::new());
    let integration_ref = integration.clone();
    let integrations: Vec<Arc<dyn crate::telemetry::TelemetryIntegration>> = vec![integration];

    let model = CallbackModelInfo::new("test", "test-model");
    let event = OnStartEvent::new("call-1", model);

    notify_start(None, &integrations, &event).await;

    assert_eq!(integration_ref.start_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_notify_finish() {
    let integration = Arc::new(CountingIntegration::new());
    let integration_ref = integration.clone();
    let integrations: Vec<Arc<dyn crate::telemetry::TelemetryIntegration>> = vec![integration];

    let step_result = StepResult::default();
    let event = crate::generate_text::OnFinishEvent::new(
        step_result,
        Vec::new(),
        vercel_ai_provider::Usage::default(),
    );

    notify_finish(None, &integrations, &event).await;

    assert_eq!(integration_ref.finish_count.load(Ordering::SeqCst), 1);
}
