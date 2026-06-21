use super::*;
use std::time::Duration;

#[test]
fn test_plain_error() {
    let err = PlainError::new("test error", StatusCode::InvalidArguments);
    assert_eq!(err.status_code(), StatusCode::InvalidArguments);
    assert_eq!(err.to_string(), "test error");
    assert!(!err.is_retryable());
}

#[test]
fn test_plain_error_retryable() {
    let err = PlainError::new("network error", StatusCode::NetworkError);
    assert!(err.is_retryable());
}

#[test]
fn test_output_msg_hides_internal() {
    let err = PlainError::new("sensitive details", StatusCode::Internal);
    assert_eq!(err.output_msg(), "Internal - sensitive details");
}

#[test]
fn test_output_msg_shows_user_errors() {
    let err = PlainError::new("Invalid parameter: foo", StatusCode::InvalidArguments);
    assert_eq!(
        err.output_msg(),
        "InvalidArguments - Invalid parameter: foo"
    );
}

#[test]
fn test_boxed_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let boxed = boxed(io_err, StatusCode::FileNotFound);

    assert_eq!(boxed.status_code(), StatusCode::FileNotFound);
    assert!(boxed.source().is_some());
}

// =========================================================================
// BoxedErrorSource tests
// =========================================================================

/// Helper error for testing ErrorExt delegation.
#[derive(Debug)]
struct RetryableError {
    status: StatusCode,
    retry_ms: Option<u64>,
    cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl std::fmt::Display for RetryableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "retryable error")
    }
}

impl std::error::Error for RetryableError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl StackError for RetryableError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: retryable error"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for RetryableError {
    fn status_code(&self) -> StatusCode {
        self.status
    }

    fn is_retryable(&self) -> bool {
        false // intentionally different from status_code().is_retryable()
    }

    fn retry_after(&self) -> Option<Duration> {
        self.retry_ms.map(Duration::from_millis)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn test_boxed_error_source_status_code_delegation() {
    let err = RetryableError {
        status: StatusCode::RateLimited,
        retry_ms: None,
        cause: None,
    };
    let boxed = boxed_err(err);
    let source = BoxedErrorSource::new(boxed);
    assert_eq!(source.status_code(), StatusCode::RateLimited);
}

#[test]
fn test_boxed_error_source_source_delegates_to_inner_source() {
    // BoxedErrorSource::source() should return the inner error's source,
    // NOT the inner error itself.
    let cause = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broken");
    let err = RetryableError {
        status: StatusCode::NetworkError,
        retry_ms: None,
        cause: Some(Box::new(cause)),
    };
    let boxed = boxed_err(err);
    let source = BoxedErrorSource::new(boxed);

    // source() should return the io::Error (inner's source), not the RetryableError
    let as_err: &dyn std::error::Error = &source;
    let chain_source = as_err.source();
    assert!(chain_source.is_some());
    assert!(
        chain_source.unwrap().to_string().contains("pipe broken"),
        "should expose inner's source error"
    );
}

#[test]
fn test_boxed_error_source_no_inner_source() {
    let err = RetryableError {
        status: StatusCode::Internal,
        retry_ms: None,
        cause: None,
    };
    let boxed = boxed_err(err);
    let source = BoxedErrorSource::new(boxed);

    let as_err: &dyn std::error::Error = &source;
    assert!(as_err.source().is_none());
}

#[test]
fn test_boxed_err_preserves_is_retryable() {
    // The inner error overrides is_retryable() to return false,
    // even though RateLimited.is_retryable() is true by default.
    let err = RetryableError {
        status: StatusCode::RateLimited,
        retry_ms: None,
        cause: None,
    };
    let boxed = boxed_err(err);
    // BoxedError (trait object) preserves the override via vtable
    assert!(
        !boxed.is_retryable(),
        "boxed_err should preserve custom is_retryable() override"
    );
}

#[test]
fn test_boxed_err_preserves_retry_after() {
    let err = RetryableError {
        status: StatusCode::RateLimited,
        retry_ms: Some(5000),
        cause: None,
    };
    let boxed = boxed_err(err);
    assert_eq!(
        boxed.retry_after(),
        Some(Duration::from_millis(5000)),
        "boxed_err should preserve retry_after()"
    );
}

#[test]
fn test_output_msg_through_boxed_error_source() {
    let cause = std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out");
    let err = RetryableError {
        status: StatusCode::NetworkError,
        retry_ms: None,
        cause: Some(Box::new(cause)),
    };
    let boxed = boxed_err(err);
    let msg = boxed.output_msg();
    // Should be "NetworkError - retryable error (connection timed out)"
    assert!(msg.contains("NetworkError"));
    assert!(msg.contains("retryable error"));
    assert!(msg.contains("connection timed out"));
}
