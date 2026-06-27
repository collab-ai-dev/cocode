use coco_types::TokenUsage;

use super::*;

#[test]
fn test_budget_tracker_continues_within_budget() {
    let mut tracker = BudgetTracker::new(Some(1000), Some(30), 3);
    tracker.record_usage(&TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 50,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 30,
            ..Default::default()
        },
    });
    assert!(matches!(tracker.check(1), BudgetDecision::Continue));
    assert_eq!(tracker.total_tokens(), 80);
}

#[test]
fn test_budget_tracker_stops_on_token_limit() {
    let mut tracker = BudgetTracker::new(Some(100), Some(30), 3);
    tracker.record_usage(&TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 80,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 30,
            ..Default::default()
        },
    });
    assert!(matches!(tracker.check(1), BudgetDecision::Stop { .. }));
}

#[test]
fn test_budget_tracker_nudges_near_limit() {
    let mut tracker = BudgetTracker::new(Some(100), Some(30), 3);
    // 92 tokens consumed, threshold is 90 (100 - 100/10)
    tracker.record_usage(&TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 52,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 40,
            ..Default::default()
        },
    });
    assert!(matches!(tracker.check(1), BudgetDecision::Nudge { .. }));
}

#[test]
fn test_budget_tracker_stops_on_turn_limit() {
    let tracker = BudgetTracker::new(Some(10_000), Some(5), 3);
    assert!(matches!(tracker.check(5), BudgetDecision::Stop { .. }));
    assert!(matches!(tracker.check(4), BudgetDecision::Continue));
}

#[test]
fn test_budget_tracker_stops_on_continuation_limit() {
    let mut tracker = BudgetTracker::new(None, Some(30), 2);
    tracker.record_continuation();
    assert!(matches!(tracker.check(1), BudgetDecision::Continue));
    tracker.record_continuation();
    assert!(matches!(tracker.check(1), BudgetDecision::Stop { .. }));
}

#[test]
fn test_budget_tracker_no_max_tokens() {
    let mut tracker = BudgetTracker::new(None, Some(30), 3);
    tracker.record_usage(&TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 999_999,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 999_999,
            ..Default::default()
        },
    });
    // No token limit, should still continue
    assert!(matches!(tracker.check(1), BudgetDecision::Continue));
}
