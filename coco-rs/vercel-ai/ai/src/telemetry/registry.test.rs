use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use super::*;
use crate::generate_text::CallbackModelInfo;
use crate::generate_text::OnStartEvent;

struct CountingIntegration {
    count: AtomicU32,
}

impl CountingIntegration {
    fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
        }
    }

    fn count(&self) -> u32 {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl crate::telemetry::TelemetryIntegration for CountingIntegration {
    async fn on_start(&self, _event: &OnStartEvent) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn test_register_and_get() {
    clear_global_integrations();

    let integration = Arc::new(CountingIntegration::new());
    register_telemetry_integration(integration);

    let integrations = get_global_integrations();
    assert_eq!(integrations.len(), 1);

    clear_global_integrations();
    let integrations = get_global_integrations();
    assert_eq!(integrations.len(), 0);
}

#[tokio::test]
async fn test_integration_receives_events() {
    clear_global_integrations();

    let integration = Arc::new(CountingIntegration::new());
    let integration_ref = integration.clone();
    register_telemetry_integration(integration);

    let model = CallbackModelInfo::new("test", "test-model");
    let event = OnStartEvent::new("call-1", model);

    let integrations = get_global_integrations();
    for i in &integrations {
        i.on_start(&event).await;
    }

    assert_eq!(integration_ref.count(), 1);

    clear_global_integrations();
}
