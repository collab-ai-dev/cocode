use super::*;
use coco_config::AutoCompactConfig;

fn auto_default() -> AutoCompactConfig {
    AutoCompactConfig::default()
}

// --- should_reactive_compact tests ---

#[test]
fn test_should_reactive_compact() {
    let config = ReactiveCompactConfig::default(); // 95% of effective window
    let auto = auto_default();
    let effective = crate::auto_trigger::effective_context_window(
        config.context_window,
        config.max_output_tokens,
        &auto,
    );
    let reactive_threshold = effective * 95 / 100;
    assert!(!should_reactive_compact(
        reactive_threshold - 1000,
        &config,
        &auto
    ));
    assert!(should_reactive_compact(
        reactive_threshold + 1000,
        &config,
        &auto
    ));
}

#[test]
fn test_calculate_drop_target() {
    let config = ReactiveCompactConfig {
        context_window: 200_000,
        ..Default::default()
    };
    let drop = calculate_drop_target(195_000, &config, &auto_default());
    assert!(drop > 0);
    assert!(drop < 195_000);
}

#[test]
fn test_below_threshold_no_compact() {
    let config = ReactiveCompactConfig {
        context_window: 200_000,
        ..Default::default()
    };
    assert!(!should_reactive_compact(100_000, &config, &auto_default()));
}

// --- ReactiveCompactState / circuit breaker tests ---

#[test]
fn test_circuit_breaker_initially_open() {
    let state = ReactiveCompactState::new();
    assert!(
        state.should_attempt_reactive_compact(),
        "fresh state should allow compaction"
    );
    assert_eq!(state.failure_count(), 0);
}

#[test]
fn test_circuit_breaker_trips_after_threshold() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    assert!(state.should_attempt_reactive_compact(), "1 failure: ok");
    state.record_failure(2000);
    assert!(state.should_attempt_reactive_compact(), "2 failures: ok");
    state.record_failure(3000);
    assert!(
        !state.should_attempt_reactive_compact(),
        "3 failures: circuit breaker should trip"
    );
    assert_eq!(state.failure_count(), 3);
    assert_eq!(state.last_attempt_ms(), 3000);
}

#[test]
fn test_circuit_breaker_reset_on_success() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    state.record_failure(2000);
    assert_eq!(state.failure_count(), 2);

    state.record_success(3000);
    assert_eq!(state.failure_count(), 0);
    assert!(
        state.should_attempt_reactive_compact(),
        "success should reset circuit breaker"
    );
    assert_eq!(state.last_attempt_ms(), 3000);
}

#[test]
fn test_circuit_breaker_reset() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    state.record_failure(2000);
    state.record_failure(3000);
    assert!(!state.should_attempt_reactive_compact());

    state.reset();
    assert!(
        state.should_attempt_reactive_compact(),
        "reset should re-enable compaction"
    );
    assert_eq!(state.failure_count(), 0);
    assert_eq!(state.last_attempt_ms(), 0);
}

#[test]
fn test_circuit_breaker_failure_after_success_restarts_count() {
    let mut state = ReactiveCompactState::new();
    state.record_failure(1000);
    state.record_failure(2000);
    state.record_success(3000);
    assert_eq!(state.failure_count(), 0);

    state.record_failure(4000);
    assert_eq!(state.failure_count(), 1);
    assert!(
        state.should_attempt_reactive_compact(),
        "single failure after reset should be ok"
    );
}

// --- AutoCompactState / rapid-refill breaker tests ---

#[test]
fn test_auto_state_initially_allows_compact() {
    let state = AutoCompactState::new();
    assert!(state.should_attempt_compact());
    assert!(!state.rapid_refill_breaker_tripped());
    assert_eq!(state.next_rapid_refill_streak(), 0);
    assert_eq!(
        state.attempt_decision(),
        AutoCompactAttemptDecision::Proceed {
            consecutive_rapid_refills: 0,
        }
    );
}

#[test]
fn test_auto_state_tracks_rapid_refills_within_three_turns() {
    let mut state = AutoCompactState::new();
    state.record_success(1000, 0);
    state.advance_turn();
    assert_eq!(state.next_rapid_refill_streak(), 1);
    assert_eq!(
        state.attempt_decision(),
        AutoCompactAttemptDecision::Proceed {
            consecutive_rapid_refills: 1,
        }
    );

    state.record_success(2000, state.next_rapid_refill_streak());
    state.advance_turn();
    assert_eq!(state.next_rapid_refill_streak(), 2);
    assert!(!state.rapid_refill_breaker_tripped());

    state.record_success(3000, state.next_rapid_refill_streak());
    state.advance_turn();
    assert_eq!(state.next_rapid_refill_streak(), 3);
    assert!(state.rapid_refill_breaker_tripped());
    assert_eq!(
        state.attempt_decision(),
        AutoCompactAttemptDecision::RapidRefillBreakerTripped {
            consecutive_rapid_refills: 3,
        }
    );
}

#[test]
fn test_auto_state_resets_rapid_refill_after_turn_window() {
    let mut state = AutoCompactState::new();
    state.record_success(1000, 2);
    state.advance_turn();
    state.advance_turn();
    assert_eq!(state.next_rapid_refill_streak(), 3);
    state.advance_turn();
    assert_eq!(state.turns_since_compact(), 3);
    assert_eq!(state.next_rapid_refill_streak(), 0);
    assert!(!state.rapid_refill_breaker_tripped());
}

#[test]
fn test_auto_state_failure_breaker_is_independent_from_rapid_refill() {
    let mut state = AutoCompactState::new();
    state.record_success(1000, 2);
    state.advance_turn();
    assert!(state.rapid_refill_breaker_tripped());

    state.record_failure(2000);
    state.record_failure(3000);
    state.record_failure(4000);
    assert!(!state.should_attempt_compact());
    assert_eq!(state.failure_count(), 3);
    assert_eq!(state.last_attempt_ms(), 4000);
    assert_eq!(
        state.attempt_decision(),
        AutoCompactAttemptDecision::FailureBreakerOpen {
            consecutive_failures: 3,
        }
    );
}

#[test]
fn test_api_microcompact_preserves_recovery_pointers() {
    use crate::types::CLEARED_TOOL_RESULT_MESSAGE;

    fn tool_result(id: &str, text: &str) -> Message {
        Message::ToolResult(coco_messages::ToolResultMessage {
            uuid: uuid::Uuid::new_v4(),
            source_assistant_uuid: None,
            display_data: None,
            message: coco_messages::LlmMessage::Tool {
                content: vec![coco_messages::ToolContent::ToolResult(
                    coco_messages::ToolResultContent {
                        tool_call_id: id.to_string(),
                        tool_name: String::new(),
                        output: coco_llm_types::ToolResultContent::text(text.to_string()),
                        is_error: false,
                        provider_metadata: None,
                    },
                )],
                provider_options: None,
            },
            tool_use_id: id.to_string(),
            tool_id: coco_types::ToolId::Builtin(coco_types::ToolName::Bash),
            is_error: false,
        })
    }

    let big_body = "x".repeat(2_000);
    let windowed = format!(
        "{big_body}\n\n[... middle omitted ...]\n\ntail\n\n<persisted-output>\nFull text saved to: /s/w.txt\n</persisted-output>"
    );
    let mut messages = vec![
        tool_result("plain", &"p".repeat(2_000)),
        tool_result("windowed", &windowed),
        tool_result(
            "reference",
            "<persisted-output>\nFull output saved to: /s/r.txt\n</persisted-output>",
        ),
    ];

    api_microcompact(&mut messages, i64::MAX);

    // Plain → bare placeholder.
    assert!(format!("{:?}", messages[0]).contains(CLEARED_TOOL_RESULT_MESSAGE));
    // Windowed → reduced, pointer retained, body freed — even under PTL pressure.
    let w = format!("{:?}", messages[1]);
    assert!(w.contains(CLEARED_TOOL_RESULT_MESSAGE));
    assert!(w.contains("/s/w.txt"));
    assert!(!w.contains(&big_body));
    // Minimal reference → untouched.
    let r = format!("{:?}", messages[2]);
    assert!(!r.contains(CLEARED_TOOL_RESULT_MESSAGE));
    assert!(r.contains("/s/r.txt"));
}
