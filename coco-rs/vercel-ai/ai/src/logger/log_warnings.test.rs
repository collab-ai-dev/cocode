//! Tests for log_warnings.rs

use super::*;

#[test]
fn test_format_warning_unsupported() {
    let warning = Warning::unsupported("tool-choice");
    let result = format_warning(&warning, "openai", "gpt-4");
    assert!(result.contains("tool-choice"));
    assert!(result.contains("not supported"));
    assert!(result.contains("openai"));
    assert!(result.contains("gpt-4"));
}

#[test]
fn test_format_warning_unsupported_with_details() {
    let warning = Warning::unsupported_with_details("tool-choice", "Use 'auto' instead");
    let result = format_warning(&warning, "openai", "gpt-4");
    assert!(result.contains("Use 'auto' instead"));
}

#[test]
fn test_format_warning_compatibility() {
    let warning = Warning::compatibility("streaming");
    let result = format_warning(&warning, "anthropic", "claude-3");
    assert!(result.contains("compatibility mode"));
}

#[test]
fn test_format_warning_other() {
    let warning = Warning::other("Something went wrong");
    let result = format_warning(&warning, "test", "model");
    assert!(result.contains("Something went wrong"));
}

#[test]
fn test_log_warnings_empty() {
    reset_log_warnings_state();
    let options = LogWarningsOptions::new(vec![], "test", "model");
    // Should not panic or log anything
    log_warnings(&options);
}

#[test]
fn test_custom_logger() {
    reset_log_warnings_state();
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();

    let logger = LogWarningsFunction::new(move |_options| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    });

    set_log_warnings(Some(logger));

    let options = LogWarningsOptions::new(vec![Warning::other("test")], "test", "model");
    log_warnings(&options);

    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Reset
    set_log_warnings(None);
}

#[test]
fn test_reset_state() {
    HAS_LOGGED_BEFORE.store(true, Ordering::Relaxed);
    reset_log_warnings_state();
    assert!(!HAS_LOGGED_BEFORE.load(Ordering::Relaxed));
}
