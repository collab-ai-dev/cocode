use pretty_assertions::assert_eq;

use super::*;
use crate::RetryConfig;

#[test]
fn test_classify_401_as_auth() {
    let err = InferenceError::from_http_status(401, "Unauthorized", None);
    assert!(matches!(err, InferenceError::AuthenticationFailed { .. }));
    assert_eq!(err.error_class(), "auth");
    assert!(!err.is_retryable());
}

#[test]
fn test_classify_429_as_rate_limit() {
    let err = InferenceError::from_http_status(429, "Too Many Requests", Some(5000));
    assert!(matches!(err, InferenceError::RateLimited { .. }));
    assert_eq!(err.retry_after_ms(), Some(5000));
    assert!(err.is_retryable());
}

#[test]
fn test_classify_503_as_overloaded() {
    let err = InferenceError::from_http_status(503, "Service Unavailable", Some(30000));
    assert!(matches!(err, InferenceError::Overloaded { .. }));
    assert!(err.is_retryable());
}

#[test]
fn test_classify_400_context_overflow() {
    let err =
        InferenceError::from_http_status(400, r#"{"error": "context_length_exceeded"}"#, None);
    assert!(matches!(err, InferenceError::ContextWindowExceeded { .. }));
    assert!(!err.is_retryable());
}

#[test]
fn test_classify_400_normal() {
    let err = InferenceError::from_http_status(400, "Bad Request", None);
    assert!(matches!(err, InferenceError::InvalidRequest { .. }));
}

#[test]
fn test_classify_retryable_status_buckets() {
    // Overload cascade (503/529): the capacity bucket, capped in-client to
    // engage fallback fast. Retryable.
    for status in [503, 529] {
        let err = InferenceError::from_http_status(status, "overloaded", None);
        assert!(
            matches!(err, InferenceError::Overloaded { .. }),
            "status {status} should be Overloaded"
        );
        assert!(err.is_retryable(), "status {status} should be retryable");
        assert!(
            RetryConfig::default().should_retry(2, &err)
                && !RetryConfig::default().should_retry(3, &err),
            "overload cascade {status} must cap at MAX_CAPACITY_RETRIES (3)"
        );
    }

    // Generic 5xx (500/502/504) + 408/409: transient, retryable with the FULL
    // backoff budget (status >= 500 / 408 / 409 up to max_retries).
    for status in [500, 502, 504, 408, 409] {
        let err = InferenceError::from_http_status(status, "server error", None);
        assert!(
            matches!(err, InferenceError::NetworkError { .. }),
            "status {status} should be NetworkError"
        );
        assert!(err.is_retryable(), "status {status} should be retryable");
        // NOT capacity-capped — gets the full retry budget.
        assert!(
            RetryConfig::default().should_retry(5, &err),
            "generic transient {status} must get the full retry budget"
        );
    }

    // 429: rate-limited, retryable, NOT capacity-capped (full budget,
    // honoring retry-after).
    let rate = InferenceError::from_http_status(429, "slow down", None);
    assert!(matches!(rate, InferenceError::RateLimited { .. }));
    assert!(rate.is_retryable());
    assert!(
        RetryConfig::default().should_retry(5, &rate),
        "429 must get the full retry budget, not the capacity cap"
    );

    // Non-retryable caller errors stay non-retryable.
    let not_found = InferenceError::from_http_status(404, "nope", None);
    assert!(matches!(not_found, InferenceError::ProviderError { .. }));
    assert!(!not_found.is_retryable());
}

#[test]
fn test_network_error_is_retryable() {
    let err = NetworkSnafu {
        message: "connection reset".to_string(),
    }
    .build();
    assert!(err.is_retryable());
    assert_eq!(err.error_class(), "network");
}

#[test]
fn test_cancelled_not_retryable() {
    let err = CancelledSnafu.build();
    assert!(!err.is_retryable());
    assert_eq!(err.error_class(), "cancelled");
}

#[test]
fn test_classify_stream_message_openai_too_many_requests_as_rate_limit() {
    // Regression: the aidp/Azure gateway delivers a 429 as an in-stream
    // SSE error frame (HTTP 200, then `{"type":"error",...}`) rather than
    // an HTTP status, so it never reaches `from_http_status`. The verbatim
    // wire blob must still classify as a retryable rate-limit so the
    // mid-stream capacity handler engages instead of bailing.
    let raw = r#"OpenAI responses error: {"type":"error","error":{"type":"too_many_requests","code":"too_many_requests","headers":{"x-ms-fe-error":"true"},"message":"Too Many Requests","param":null},"sequence_number":2}"#;
    let err = InferenceError::classify_stream_message(raw)
        .expect("too_many_requests must classify as a rate-limit");
    assert!(matches!(err, InferenceError::RateLimited { .. }));
    assert!(err.is_retryable());
}

#[test]
fn test_classify_stream_message_vocabulary() {
    // Anthropic streams `rate_limit_error`; the human-readable "rate
    // limited" and the `(429)` / `status: 429` forms must all match.
    for msg in [
        "rate_limit_error: too fast",
        "you are being rate limited",
        "Too Many Requests",
        "provider returned status: 429",
        "error (429)",
    ] {
        assert!(
            matches!(
                InferenceError::classify_stream_message(msg),
                Some(InferenceError::RateLimited { .. })
            ),
            "{msg:?} should classify as RateLimited"
        );
    }
    // Overload + context-window vocabulary stays in its own bucket.
    assert!(matches!(
        InferenceError::classify_stream_message("overloaded_error"),
        Some(InferenceError::Overloaded { .. })
    ));
    assert!(matches!(
        InferenceError::classify_stream_message("context_length_exceeded"),
        Some(InferenceError::ContextWindowExceeded { .. })
    ));
    // Unrelated errors don't get a recoverable classification.
    assert!(InferenceError::classify_stream_message("invalid api key").is_none());
}

#[test]
fn test_body_truncation() {
    let long_body = "x".repeat(1000);
    // 404 maps to the message-bearing ProviderError variant (500 now maps to
    // the retryable Overloaded bucket, which carries no body).
    let err = InferenceError::from_http_status(404, &long_body, None);
    if let InferenceError::ProviderError { message, .. } = &err {
        assert!(message.len() <= 504); // 500 + "..."
        assert!(message.ends_with("..."));
    } else {
        panic!("expected ProviderError for 404");
    }
}
