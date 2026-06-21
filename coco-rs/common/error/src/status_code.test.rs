use super::*;
use strum::IntoEnumIterator;

#[test]
fn test_status_code_values() {
    // General categories (01-05)
    assert_eq!(StatusCode::Success as i32, 0);
    assert_eq!(StatusCode::Unknown as i32, 1_000);
    assert_eq!(StatusCode::InvalidArguments as i32, 2_000);
    assert_eq!(StatusCode::IoError as i32, 3_000);
    assert_eq!(StatusCode::NetworkError as i32, 4_000);
    assert_eq!(StatusCode::AuthenticationFailed as i32, 5_000);

    // Business categories (10-12)
    assert_eq!(StatusCode::InvalidConfig as i32, 10_000);
    assert_eq!(StatusCode::ProviderNotFound as i32, 11_000);
    assert_eq!(StatusCode::RateLimited as i32, 12_000);
}

#[test]
fn test_is_success() {
    assert!(StatusCode::is_success(0));
    assert!(!StatusCode::is_success(1_000));
}

#[test]
fn test_is_retryable() {
    assert!(StatusCode::NetworkError.is_retryable());
    assert!(StatusCode::RateLimited.is_retryable());
    assert!(StatusCode::Timeout.is_retryable());
    assert!(!StatusCode::InvalidArguments.is_retryable());
    assert!(!StatusCode::AuthenticationFailed.is_retryable());
}

#[test]
fn test_should_log_error() {
    assert!(StatusCode::Unknown.should_log_error());
    assert!(StatusCode::Internal.should_log_error());
    assert!(!StatusCode::InvalidArguments.should_log_error());
}

#[test]
fn test_display() {
    assert_eq!(format!("{}", StatusCode::Success), "Success");
    assert_eq!(format!("{}", StatusCode::NetworkError), "NetworkError");
}

#[test]
fn test_name() {
    assert_eq!(StatusCode::Success.name(), "Success");
    assert_eq!(StatusCode::NetworkError.name(), "NetworkError");
    assert_eq!(StatusCode::InvalidArguments.name(), "InvalidArguments");
    assert_eq!(
        StatusCode::AuthenticationFailed.name(),
        "AuthenticationFailed"
    );
}

#[test]
fn test_category() {
    // General categories
    assert_eq!(StatusCode::Success.category(), StatusCategory::Success);
    assert_eq!(StatusCode::Unknown.category(), StatusCategory::Common);
    assert_eq!(
        StatusCode::InvalidArguments.category(),
        StatusCategory::Input
    );
    assert_eq!(StatusCode::IoError.category(), StatusCategory::IO);
    assert_eq!(StatusCode::NetworkError.category(), StatusCategory::Network);
    assert_eq!(
        StatusCode::AuthenticationFailed.category(),
        StatusCategory::Auth
    );

    // Business categories
    assert_eq!(StatusCode::InvalidConfig.category(), StatusCategory::Config);
    assert_eq!(
        StatusCode::ProviderNotFound.category(),
        StatusCategory::Provider
    );
    assert_eq!(StatusCode::RateLimited.category(), StatusCategory::Resource);
}

#[test]
fn test_metadata_consistency() {
    for code in StatusCode::iter() {
        let meta = code.meta();
        let value = code as i32;

        // Verify category matches code range (XX_YYY format)
        match meta.category {
            StatusCategory::Success => assert_eq!(value, 0),
            StatusCategory::Common => assert!((1_000..2_000).contains(&value)),
            StatusCategory::Input => assert!((2_000..3_000).contains(&value)),
            StatusCategory::IO => assert!((3_000..4_000).contains(&value)),
            StatusCategory::Network => assert!((4_000..5_000).contains(&value)),
            StatusCategory::Auth => assert!((5_000..6_000).contains(&value)),
            StatusCategory::Config => assert!((10_000..11_000).contains(&value)),
            StatusCategory::Provider => assert!((11_000..12_000).contains(&value)),
            StatusCategory::Resource => assert!((12_000..13_000).contains(&value)),
            StatusCategory::SystemReminder => assert!((13_000..14_000).contains(&value)),
        }
    }
}

#[test]
fn test_retryable_rules() {
    // All network errors should be retryable
    assert!(StatusCode::NetworkError.is_retryable());
    assert!(StatusCode::ConnectionFailed.is_retryable());
    assert!(StatusCode::ServiceUnavailable.is_retryable());

    // Rate limits and timeouts should be retryable
    assert!(StatusCode::RateLimited.is_retryable());
    assert!(StatusCode::Timeout.is_retryable());
    assert!(StatusCode::ResourcesExhausted.is_retryable());

    // Internal errors might be transient
    assert!(StatusCode::Internal.is_retryable());

    // Stream errors can be retried
    assert!(StatusCode::StreamError.is_retryable());

    // Auth errors should NOT be retryable
    assert!(!StatusCode::AuthenticationFailed.is_retryable());
    assert!(!StatusCode::PermissionDenied.is_retryable());

    // Input errors should NOT be retryable
    assert!(!StatusCode::InvalidArguments.is_retryable());
    assert!(!StatusCode::InvalidConfig.is_retryable());

    // QuotaExceeded should NOT be retryable (needs user action)
    assert!(!StatusCode::QuotaExceeded.is_retryable());
}

#[test]
fn test_log_error_rules() {
    // Unexpected errors should be logged
    assert!(StatusCode::Unknown.should_log_error());
    assert!(StatusCode::Internal.should_log_error());
    assert!(StatusCode::External.should_log_error());

    // Provider errors should be logged for debugging
    assert!(StatusCode::ProviderError.should_log_error());
    assert!(StatusCode::StreamError.should_log_error());

    // User errors should NOT be logged
    assert!(!StatusCode::InvalidArguments.should_log_error());
    assert!(!StatusCode::AuthenticationFailed.should_log_error());
}
