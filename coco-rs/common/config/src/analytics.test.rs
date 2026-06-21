use super::*;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

/// Test sink that captures logged events.
struct TestSink {
    events: StdMutex<Vec<(String, EventProperties)>>,
}

impl TestSink {
    fn new() -> Self {
        Self {
            events: StdMutex::new(Vec::new()),
        }
    }

    fn event_count(&self) -> usize {
        self.events.lock().expect("test lock").len()
    }

    fn event_names(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("test lock")
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }
}

impl AnalyticsSink for TestSink {
    fn log_event(&self, event_name: &str, properties: &EventProperties) {
        self.events
            .lock()
            .expect("test lock")
            .push((event_name.to_string(), properties.clone()));
    }
}

#[test]
fn test_pipeline_queues_before_sink() {
    let pipeline = AnalyticsPipeline::new();
    let mut props = EventProperties::new();
    props.insert(
        "model".into(),
        MetadataValue::Str("claude-sonnet-4-20250514".into()),
    );
    pipeline.log_event("api_query", props);

    assert_eq!(pipeline.queued_count(), 1);
}

#[test]
fn test_pipeline_drains_on_sink_attach() {
    let pipeline = AnalyticsPipeline::new();
    pipeline.log_event("event_1", EventProperties::new());
    pipeline.log_event("event_2", EventProperties::new());
    assert_eq!(pipeline.queued_count(), 2);

    let sink = Arc::new(TestSink::new());
    pipeline.attach_sink(Arc::clone(&sink) as Arc<dyn AnalyticsSink>);

    assert_eq!(pipeline.queued_count(), 0);
    assert_eq!(sink.event_count(), 2);
    assert_eq!(sink.event_names(), vec!["event_1", "event_2"]);
}

#[test]
fn test_pipeline_direct_dispatch_with_sink() {
    let pipeline = AnalyticsPipeline::new();
    let sink = Arc::new(TestSink::new());
    pipeline.attach_sink(Arc::clone(&sink) as Arc<dyn AnalyticsSink>);

    let mut props = EventProperties::new();
    props.insert("tokens".into(), MetadataValue::Int(5000));
    pipeline.log_event("api_success", props);

    assert_eq!(pipeline.queued_count(), 0);
    assert_eq!(sink.event_count(), 1);

    let events = sink.events.lock().expect("lock");
    assert_eq!(events[0].0, "api_success");
    assert_eq!(events[0].1["tokens"], MetadataValue::Int(5000));
}

#[test]
fn test_pipeline_attach_idempotent() {
    let pipeline = AnalyticsPipeline::new();
    let sink1 = Arc::new(TestSink::new());
    let sink2 = Arc::new(TestSink::new());

    pipeline.attach_sink(Arc::clone(&sink1) as Arc<dyn AnalyticsSink>);
    pipeline.attach_sink(Arc::clone(&sink2) as Arc<dyn AnalyticsSink>);

    pipeline.log_event("test", EventProperties::new());
    // Events go to sink1 (first attached), not sink2
    assert_eq!(sink1.event_count(), 1);
    assert_eq!(sink2.event_count(), 0);
}

#[test]
fn test_session_counters() {
    let pipeline = AnalyticsPipeline::new();
    pipeline.increment_counter("api_calls", 1);
    pipeline.increment_counter("api_calls", 1);
    pipeline.increment_counter("total_tokens", 5000);
    pipeline.increment_counter("total_tokens", 3000);

    assert_eq!(pipeline.counter_value("api_calls"), 2);
    assert_eq!(pipeline.counter_value("total_tokens"), 8000);
    assert_eq!(pipeline.counter_value("nonexistent"), 0);

    let counters = pipeline.session_counters();
    assert_eq!(counters.len(), 2);
}

#[test]
fn test_feature_flags() {
    let pipeline = AnalyticsPipeline::new();
    let mut flags = HashMap::new();
    flags.insert("enable_datadog".into(), serde_json::json!(true));
    flags.insert("sample_rate".into(), serde_json::json!(0.5));
    flags.insert(
        "model_override".into(),
        serde_json::json!("claude-opus-4-20250514"),
    );
    pipeline.set_feature_flags(flags);

    assert!(pipeline.check_feature_gate("enable_datadog"));
    assert!(!pipeline.check_feature_gate("nonexistent_gate"));
    assert_eq!(pipeline.get_feature_value::<f64>("sample_rate", 1.0), 0.5);
    assert_eq!(
        pipeline.get_feature_value::<String>("model_override", "default".into()),
        "claude-opus-4-20250514"
    );
    assert_eq!(
        pipeline.get_feature_value::<String>("missing_key", "fallback".into()),
        "fallback"
    );
}

#[test]
fn test_session_analytics_cache_hit_rate() {
    let analytics = SessionAnalytics {
        total_input_tokens: 100_000,
        total_cache_read_tokens: 80_000,
        ..Default::default()
    };
    let rate = analytics.cache_hit_rate();
    assert!((rate - 80.0).abs() < 0.01);
}

#[test]
fn test_session_analytics_zero_calls() {
    let analytics = SessionAnalytics::default();
    assert_eq!(analytics.cache_hit_rate(), 0.0);
    assert_eq!(analytics.avg_tokens_per_call(), 0.0);
}

#[test]
fn test_session_analytics_avg_tokens() {
    let analytics = SessionAnalytics {
        api_call_count: 10,
        total_input_tokens: 50_000,
        total_output_tokens: 10_000,
        ..Default::default()
    };
    assert_eq!(analytics.avg_tokens_per_call(), 6000.0);
}

#[test]
fn test_session_analytics_to_properties() {
    let analytics = SessionAnalytics {
        api_call_count: 5,
        total_input_tokens: 10000,
        total_output_tokens: 2000,
        total_cache_read_tokens: 8000,
        total_cache_creation_tokens: 2000,
        total_api_duration_ms: 5000,
        error_count: 1,
        retry_count: 2,
        session_duration_ms: 60_000,
        tool_call_count: 15,
        total_cost_usd: 0.05,
    };
    let props = analytics.to_properties();
    assert_eq!(props["api_call_count"], MetadataValue::Int(5));
    assert_eq!(props["total_input_tokens"], MetadataValue::Int(10000));
    assert_eq!(props["error_count"], MetadataValue::Int(1));
    assert_eq!(props["total_cost_usd"], MetadataValue::Float(0.05));
    // cache_hit_rate = 80%
    assert_eq!(props["cache_hit_rate"], MetadataValue::Float(80.0));
}

#[test]
fn test_sanitize_tool_name() {
    assert_eq!(sanitize_tool_name("Read"), "Read");
    assert_eq!(sanitize_tool_name("Bash"), "Bash");
    assert_eq!(sanitize_tool_name("mcp__github__list_repos"), "mcp_tool");
    assert_eq!(sanitize_tool_name("mcp__custom_server__tool"), "mcp_tool");
}

#[test]
fn test_strip_proto_fields() {
    let mut props = EventProperties::new();
    props.insert("model".into(), MetadataValue::Str("test".into()));
    props.insert(
        "_PROTO_user_name".into(),
        MetadataValue::Str("secret".into()),
    );
    props.insert("tokens".into(), MetadataValue::Int(100));
    props.insert(
        "_PROTO_email".into(),
        MetadataValue::Str("email@example.com".into()),
    );

    let stripped = strip_proto_fields(&props);
    assert_eq!(stripped.len(), 2);
    assert!(stripped.contains_key("model"));
    assert!(stripped.contains_key("tokens"));
    assert!(!stripped.contains_key("_PROTO_user_name"));
    assert!(!stripped.contains_key("_PROTO_email"));
}

#[test]
fn test_strip_proto_fields_no_proto_keys() {
    let mut props = EventProperties::new();
    props.insert("model".into(), MetadataValue::Str("test".into()));
    props.insert("tokens".into(), MetadataValue::Int(100));

    let stripped = strip_proto_fields(&props);
    assert_eq!(stripped, props);
}

#[test]
fn test_metadata_value_from_impls() {
    assert_eq!(MetadataValue::from(true), MetadataValue::Bool(true));
    assert_eq!(MetadataValue::from(42_i64), MetadataValue::Int(42));
    assert_eq!(MetadataValue::from(2.5_f64), MetadataValue::Float(2.5));
    assert_eq!(
        MetadataValue::from("hello"),
        MetadataValue::Str("hello".into())
    );
}

#[test]
fn test_pipeline_reset() {
    let pipeline = AnalyticsPipeline::new();
    let sink = Arc::new(TestSink::new());
    pipeline.attach_sink(Arc::clone(&sink) as Arc<dyn AnalyticsSink>);
    pipeline.increment_counter("count", 5);
    pipeline.set_feature_flags(HashMap::from([("flag".into(), serde_json::json!(true))]));

    pipeline.reset();

    assert_eq!(pipeline.queued_count(), 0);
    assert_eq!(pipeline.counter_value("count"), 0);
    assert!(!pipeline.check_feature_gate("flag"));

    // After reset, events queue again (no sink)
    pipeline.log_event("after_reset", EventProperties::new());
    assert_eq!(pipeline.queued_count(), 1);
}

#[test]
fn test_pipeline_clone_shares_state() {
    let pipeline = AnalyticsPipeline::new();
    let clone = pipeline.clone();

    pipeline.increment_counter("shared", 10);
    assert_eq!(clone.counter_value("shared"), 10);

    clone.log_event("from_clone", EventProperties::new());
    assert_eq!(pipeline.queued_count(), 1);
}

#[test]
fn test_metric_event_serialization() {
    let metric = MetricEvent {
        name: "api_latency_ms".into(),
        value: 1200.5,
        tags: HashMap::from([("model".into(), "claude-sonnet-4-20250514".into())]),
        timestamp_ms: 1700000000000,
    };
    let json = serde_json::to_string(&metric).expect("serialize");
    assert!(json.contains("api_latency_ms"));
    assert!(json.contains("1200.5"));
    assert!(json.contains("claude-sonnet-4-20250514"));
}

#[test]
fn test_sampling_zero_rate_drops_event() {
    let pipeline = AnalyticsPipeline::new();
    let sink = Arc::new(TestSink::new());
    pipeline.attach_sink(Arc::clone(&sink) as Arc<dyn AnalyticsSink>);

    pipeline.set_sampling_config(HashMap::from([("sampled_event".into(), 0.0)]));

    pipeline.log_event("sampled_event", EventProperties::new());
    assert_eq!(sink.event_count(), 0);
}

#[test]
fn test_sampling_full_rate_passes_event() {
    let pipeline = AnalyticsPipeline::new();
    let sink = Arc::new(TestSink::new());
    pipeline.attach_sink(Arc::clone(&sink) as Arc<dyn AnalyticsSink>);

    pipeline.set_sampling_config(HashMap::from([("always_event".into(), 1.0)]));

    pipeline.log_event("always_event", EventProperties::new());
    assert_eq!(sink.event_count(), 1);
}
