use super::*;
use opentelemetry::trace::SpanId;
use opentelemetry::trace::TraceContextExt;
use opentelemetry::trace::TraceId;

#[test]
fn parses_valid_traceparent() {
    let trace_id = "00000000000000000000000000000001";
    let span_id = "0000000000000002";
    let context = extract_traceparent_context(format!("00-{trace_id}-{span_id}-01"), None)
        .expect("trace context");
    let span = context.span();
    let span_context = span.span_context();
    assert_eq!(
        span_context.trace_id(),
        TraceId::from_hex(trace_id).unwrap()
    );
    assert_eq!(span_context.span_id(), SpanId::from_hex(span_id).unwrap());
    assert!(span_context.is_remote());
}

#[test]
fn invalid_traceparent_returns_none() {
    assert!(extract_traceparent_context("not-a-traceparent".to_string(), None).is_none());
}
