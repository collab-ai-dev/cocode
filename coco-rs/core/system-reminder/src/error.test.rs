use super::system_reminder_error::*;
use coco_error::ErrorExt;
use coco_error::StatusCode;
use pretty_assertions::assert_eq;

#[test]
fn generator_timeout_maps_to_13_000() {
    let err: super::SystemReminderError = GeneratorTimeoutSnafu {
        generator: "Plan".to_string(),
        timeout_ms: 1000_i64,
    }
    .build();
    assert_eq!(err.status_code(), StatusCode::ReminderGeneratorTimeout);
    assert_eq!(err.status_code() as i32, 13_000);
}

#[test]
fn generator_failed_maps_to_13_001() {
    let err: super::SystemReminderError = GeneratorFailedSnafu {
        generator: "Todo".to_string(),
        message: "boom".to_string(),
    }
    .build();
    assert_eq!(err.status_code(), StatusCode::ReminderGeneratorFailed);
}

#[test]
fn throttle_poisoned_maps_to_13_002() {
    let err: super::SystemReminderError = ThrottlePoisonedSnafu {
        attachment_type: "PlanMode".to_string(),
    }
    .build();
    assert_eq!(err.status_code(), StatusCode::ReminderThrottlePoisoned);
}

#[test]
fn invalid_context_maps_to_13_003() {
    let err: super::SystemReminderError = InvalidContextSnafu {
        generator: "PlanMode".to_string(),
        field: "plan_file_path".to_string(),
    }
    .build();
    assert_eq!(err.status_code(), StatusCode::ReminderInvalidContext);
}

#[test]
fn errors_are_not_retryable() {
    let err: super::SystemReminderError = GeneratorTimeoutSnafu {
        generator: "Plan".to_string(),
        timeout_ms: 1000_i64,
    }
    .build();
    assert!(!err.status_code().is_retryable());
}
