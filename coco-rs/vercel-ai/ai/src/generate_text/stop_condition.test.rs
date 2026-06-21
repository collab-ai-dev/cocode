//! Tests for stop_condition.rs

use super::*;
use crate::generate_text::StepResult;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::Usage;

#[test]
fn test_step_count_description() {
    let condition = step_count_is(3);
    assert!(condition.description().contains("3"));
}

#[test]
fn test_is_step_count_matches_exact_count_only() {
    let condition = is_step_count(2);
    let steps = [make_step(0), make_step(1), make_step(2)];

    assert!(!condition.is_met(&steps[..1]));
    assert!(condition.is_met(&steps[..2]));
    assert!(!condition.is_met(&steps[..3]));
}

#[test]
fn test_has_tool_call_description() {
    let condition = has_tool_call("my_tool");
    assert!(condition.description().contains("my_tool"));
}

#[test]
fn test_response_contains_description() {
    let condition = response_contains("hello");
    assert!(condition.description().contains("hello"));
}

fn make_step(step: u32) -> StepResult {
    StepResult::new(
        step,
        String::new(),
        Usage::default(),
        FinishReason::end_turn(),
    )
}
