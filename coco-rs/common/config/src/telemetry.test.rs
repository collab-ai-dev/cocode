use super::*;

#[test]
fn test_logger_disabled() {
    let mut logger = AnalyticsLogger::new(
        TelemetryConfig {
            enabled: false,
            ..Default::default()
        },
        "test".into(),
    );
    logger.log_tool_use("Read", 100, false);
    assert_eq!(logger.pending_count(), 0);
}

#[test]
fn test_logger_enabled() {
    let mut logger = AnalyticsLogger::new(
        TelemetryConfig {
            enabled: true,
            ..Default::default()
        },
        "test-session".into(),
    );
    logger.log_tool_use("Bash", 500, false);
    logger.log_permission("Write", "allow", "safe path");
    assert_eq!(logger.pending_count(), 2);

    let events = logger.drain_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_name, "tool_use");
    assert_eq!(events[1].event_name, "permission_decision");
    assert_eq!(logger.pending_count(), 0);
}
