use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_default_budget_matches_documented_defaults() {
    let budget = GoalBudget::default();
    assert_eq!(budget.max_autonomous_turns.get(), 20);
    assert_eq!(budget.probe_interval.get(), 5);
    assert!(budget.max_tokens.is_none());
}

#[test]
fn test_usage_apply_folds_tokens_and_duration() {
    let mut usage = GoalUsage::default();
    usage.apply(UsageDelta {
        input_tokens: 100,
        output_tokens: 40,
        duration_ms: 500,
    });
    usage.apply(UsageDelta {
        input_tokens: 10,
        output_tokens: 4,
        duration_ms: 50,
    });
    assert_eq!(usage.input_tokens, 110);
    assert_eq!(usage.output_tokens, 44);
    assert_eq!(usage.total_tokens(), 154);
    assert_eq!(usage.active_duration_ms, 550);
}

#[test]
fn test_autonomous_exhausted_at_cap() {
    let budget = GoalBudget::default();
    let mut counters = GoalCounters {
        autonomous_turns: 19,
        ..Default::default()
    };
    assert!(!counters.autonomous_exhausted(&budget));
    counters.autonomous_turns = 20;
    assert!(counters.autonomous_exhausted(&budget));
}

#[test]
fn test_no_progress_tripped_at_limit() {
    let below = GoalCounters {
        no_progress_streak: NO_PROGRESS_LIMIT - 1,
        ..Default::default()
    };
    assert!(!below.no_progress_tripped());
    let at_limit = GoalCounters {
        no_progress_streak: NO_PROGRESS_LIMIT,
        ..Default::default()
    };
    assert!(at_limit.no_progress_tripped());
}

#[test]
fn test_probe_due_respects_cadence_and_cooldown() {
    let budget = GoalBudget::default();
    let mut counters = GoalCounters {
        continuations_since_user_turn: 5,
        ..Default::default()
    };
    assert!(counters.probe_due(&budget));
    counters.probe_cooldown = true;
    assert!(!counters.probe_due(&budget));
    counters.probe_cooldown = false;
    counters.continuations_since_user_turn = 4;
    assert!(!counters.probe_due(&budget));
}

#[test]
fn test_trigger_quota_and_cadence_semantics() {
    assert!(GoalTurnTrigger::Autonomous.spends_autonomous_quota());
    assert!(!GoalTurnTrigger::UserInput.spends_autonomous_quota());
    assert!(!GoalTurnTrigger::Creation.spends_autonomous_quota());
    assert!(GoalTurnTrigger::UserInput.resets_probe_cadence());
    assert!(GoalTurnTrigger::Creation.resets_probe_cadence());
    assert!(!GoalTurnTrigger::Autonomous.resets_probe_cadence());
}
