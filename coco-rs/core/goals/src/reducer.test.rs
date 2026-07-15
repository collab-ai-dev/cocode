use crate::test_support::*;
use crate::*;
use pretty_assertions::assert_eq;

fn start(
    snapshot: &GoalSnapshot,
    lease_name: &str,
    turn_name: &str,
    trigger: GoalTurnTrigger,
) -> GoalSnapshot {
    next_snapshot(
        Some(snapshot),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease(lease_name),
            turn_id: turn(turn_name),
            trigger,
            at: ts(10),
        }),
    )
}

fn finish_continue(
    snapshot: &GoalSnapshot,
    lease_name: &str,
    turn_name: &str,
    next_lease: &str,
    signals: Vec<ProgressSignal>,
) -> GoalDecision {
    apply(
        Some(snapshot),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease(lease_name),
            turn_id: turn(turn_name),
            reported: true,
            signals,
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Continue {
                next_lease_id: lease(next_lease),
                checkpoint: None,
                rejection: None,
            },
            at: ts(20),
        }),
    )
}

// ── Creation ──────────────────────────────────────────────────────────────

#[test]
fn test_create_yields_active_with_queued_lease_and_schedule_effect() {
    let decision = apply(None, GoalCommand::Create(create_cmd()));
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    match &snapshot.lifecycle {
        GoalLifecycle::Active {
            lease: GoalLease::Queued { lease_id, .. },
        } => assert_eq!(lease_id, &lease("l0")),
        other => panic!("expected active(queued), got {other:?}"),
    }
    assert!(decision.effects.contains(&GoalEffect::ScheduleTurn {
        lease_id: lease("l0")
    }));
    assert_eq!(decision.event, GoalTransitionEvent::Created);
}

#[test]
fn test_create_while_mode_gated_yields_waiting_with_wake() {
    let mut cmd = create_cmd();
    cmd.mode_gate = Some(ModeGate::Plan);
    let decision = apply(None, GoalCommand::Create(cmd));
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Waiting);
    let wake = snapshot.lifecycle.wake().expect("waiting carries a wake");
    assert!(matches!(
        wake.condition,
        WaitCondition::ModeGate {
            mode: ModeGate::Plan
        }
    ));
    assert!(decision.effects.iter().any(|e| matches!(
        e,
        GoalEffect::RegisterWake { wake } if matches!(wake.condition, WaitCondition::ModeGate { .. })
    )));
}

#[test]
fn test_create_rejects_when_active_goal_exists() {
    let existing = created_snapshot();
    let err = decide(Some(&existing), GoalCommand::Create(create_cmd())).unwrap_err();
    assert_eq!(err, GoalTransitionError::GoalAlreadyActive);
}

#[test]
fn test_create_rejects_policy_that_cannot_judge_contract() {
    let mut cmd = create_cmd();
    cmd.policy = CompletionPolicy::ContractChecks;
    cmd.contract = Some(CompletionContract {
        items: vec![ContractItem::Criterion(SemanticCriterion {
            claim: BoundedText::short("looks right"),
            anchor: None,
        })],
        referenced_docs: Vec::new(),
        approved_at_spec: SpecRevision::INITIAL,
    });
    let err = decide(None, GoalCommand::Create(cmd)).unwrap_err();
    assert_eq!(err, GoalTransitionError::InvalidPolicyForContract);
}

// ── Turn start ────────────────────────────────────────────────────────────

#[test]
fn test_start_turn_queued_to_running_increments_total() {
    let created = created_snapshot();
    let started = start(&created, "l0", "t0", GoalTurnTrigger::Creation);
    assert_eq!(started.counters.total_turns, 1);
    assert_eq!(started.counters.autonomous_turns, 0);
    assert!(matches!(
        started.lifecycle,
        GoalLifecycle::Active {
            lease: GoalLease::Running { .. }
        }
    ));
}

#[test]
fn test_autonomous_start_spends_quota_and_advances_probe_cadence() {
    let created = created_snapshot();
    let started = start(&created, "l0", "t0", GoalTurnTrigger::Autonomous);
    assert_eq!(started.counters.autonomous_turns, 1);
    assert_eq!(started.counters.continuations_since_user_turn, 1);
}

#[test]
fn test_user_start_resets_probe_cadence() {
    let mut created = created_snapshot();
    created.counters.continuations_since_user_turn = 4;
    created.counters.probe_cooldown = true;
    let started = start(&created, "l0", "t0", GoalTurnTrigger::UserInput);
    assert_eq!(started.counters.continuations_since_user_turn, 0);
    assert!(!started.counters.probe_cooldown);
}

#[test]
fn test_start_turn_lease_mismatch_rejected() {
    let created = created_snapshot();
    let err = decide(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease("wrong"),
            turn_id: turn("t0"),
            trigger: GoalTurnTrigger::Creation,
            at: ts(10),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::LeaseMismatch);
}

#[test]
fn test_autonomous_start_at_cap_enters_budget_limited_turns() {
    let mut created = created_snapshot();
    created.counters.autonomous_turns = created.budget.max_autonomous_turns.get();
    let decision = apply(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            trigger: GoalTurnTrigger::Autonomous,
            at: ts(10),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert!(matches!(
        snapshot.lifecycle,
        GoalLifecycle::BudgetLimited {
            kind: BudgetKind::Turns,
            ..
        }
    ));
}

// ── Turn finalize ─────────────────────────────────────────────────────────

#[test]
fn test_finish_continue_queues_next_lease() {
    let running = running_snapshot();
    let decision = finish_continue(
        &running,
        "l0",
        "t0",
        "l1",
        vec![ProgressSignal::ToolObservation],
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    match &snapshot.lifecycle {
        GoalLifecycle::Active {
            lease: GoalLease::Queued { lease_id, .. },
        } => assert_eq!(lease_id, &lease("l1")),
        other => panic!("expected active(queued l1), got {other:?}"),
    }
    assert_eq!(snapshot.counters.no_progress_streak, 0);
    assert!(decision.effects.contains(&GoalEffect::ScheduleTurn {
        lease_id: lease("l1")
    }));
    // A normal continue rolls the lease forward; it is not a terminal transition,
    // so no lease-scoped resources are released (§9.5).
    assert!(
        !decision
            .effects
            .iter()
            .any(|e| matches!(e, GoalEffect::ReleaseLease { .. }))
    );
}

#[test]
fn test_finish_continue_token_exhausted_enters_budget_limited() {
    let mut created = created_snapshot();
    created.budget.max_tokens = std::num::NonZeroU64::new(100);
    let running = start(&created, "l0", "t0", GoalTurnTrigger::Creation);
    let decision = apply(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: vec![ProgressSignal::ToolObservation],
            usage: UsageDelta {
                input_tokens: 90,
                output_tokens: 20,
                duration_ms: 0,
            },
            outcome: TurnFinishOutcome::Continue {
                next_lease_id: lease("l1"),
                checkpoint: None,
                rejection: None,
            },
            at: ts(20),
        }),
    );
    assert!(matches!(
        decision.snapshot.as_ref().unwrap().lifecycle,
        GoalLifecycle::BudgetLimited {
            kind: BudgetKind::Tokens,
            ..
        }
    ));
}

#[test]
fn test_three_signal_free_continues_pause_no_progress() {
    let s0 = running_snapshot();
    let s1 = finish_continue(&s0, "l0", "t0", "l1", Vec::new())
        .snapshot
        .unwrap();
    assert_eq!(s1.counters.no_progress_streak, 1);
    let s1r = start(&s1, "l1", "t1", GoalTurnTrigger::UserInput);
    let s2 = finish_continue(&s1r, "l1", "t1", "l2", Vec::new())
        .snapshot
        .unwrap();
    assert_eq!(s2.counters.no_progress_streak, 2);
    let s2r = start(&s2, "l2", "t2", GoalTurnTrigger::UserInput);
    let s3 = finish_continue(&s2r, "l2", "t2", "l3", Vec::new())
        .snapshot
        .unwrap();
    assert!(matches!(
        s3.lifecycle,
        GoalLifecycle::Paused {
            reason: PauseReason::NoProgress
        }
    ));
}

#[test]
fn test_unreported_streak_tracks_separately_from_progress() {
    let running = running_snapshot();
    // Real signal, but no report → progress resets, unreported increments.
    let decision = apply(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: false,
            signals: vec![ProgressSignal::WorkspaceChange],
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Continue {
                next_lease_id: lease("l1"),
                checkpoint: None,
                rejection: None,
            },
            at: ts(20),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.counters.no_progress_streak, 0);
    assert_eq!(snapshot.counters.unreported_streak, 1);
}

#[test]
fn test_finish_wait_enters_waiting_with_wake_and_register_effect() {
    let running = running_snapshot();
    let decision = apply(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: vec![ProgressSignal::WaitRegistered],
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Wait {
                wake_id: wake("w1"),
                condition: WaitCondition::Deadline {
                    deadline: Timestamp::from_millis(9999),
                },
            },
            at: ts(20),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Waiting);
    assert!(decision.effects.iter().any(|e| matches!(
        e,
        GoalEffect::RegisterWake { wake } if wake.wake_id == WakeId::new("w1")
    )));
    assert!(decision.effects.contains(&GoalEffect::ReleaseLease {
        lease_id: lease("l0")
    }));
}

#[test]
fn test_finish_blocked_records_blocker() {
    let running = running_snapshot();
    let decision = apply(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: Vec::new(),
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Blocked {
                evidence: BlockerEvidence::ExecutionError {
                    message: BoundedText::short("compiler crashed"),
                },
            },
            at: ts(20),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Blocked);
    assert!(snapshot.last_blocker.is_some());
}

#[test]
fn test_finish_completed_with_valid_authorization() {
    let running = running_snapshot();
    let auth = authorization_for(&running, &lease("l0"));
    let decision = apply(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: Vec::new(),
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Completed {
                authorization: auth,
            },
            at: ts(20),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Completed);
    assert!(snapshot.is_terminal());
    // Terminal completion schedules no further work.
    assert!(
        !decision
            .effects
            .iter()
            .any(|e| matches!(e, GoalEffect::ScheduleTurn { .. }))
    );
}

#[test]
fn test_finish_completed_with_stale_spec_authorization_rejected() {
    let running = running_snapshot();
    let auth = authorization_for(&running, &lease("l0"));
    // Advance spec so the authorization no longer matches.
    let edited = next_snapshot(
        Some(&running),
        GoalCommand::Edit(Edit {
            goal_id: goal_id(),
            expected_spec_revision: SpecRevision::INITIAL,
            objective: Some(GoalObjective::new("changed")),
            contract: None,
            clear_contract: false,
            policy: None,
            budget: None,
            plan_binding: None,
            next_lease_id: None,
            at: ts(15),
        }),
    );
    let err = decide(
        Some(&edited),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: Vec::new(),
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Completed {
                authorization: auth,
            },
            at: ts(20),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::CompletionAuthorizationMismatch);
}

#[test]
fn test_finish_turn_mismatch_rejected() {
    let running = running_snapshot();
    let err = decide(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("wrong-turn"),
            reported: true,
            signals: Vec::new(),
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Continue {
                next_lease_id: lease("l1"),
                checkpoint: None,
                rejection: None,
            },
            at: ts(20),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::TurnMismatch);
}

// ── Wake / pause / resume ────────────────────────────────────────────────

#[test]
fn test_wake_returns_to_active_with_resolution_and_reminder() {
    let running = running_snapshot();
    let waiting = next_snapshot(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: vec![ProgressSignal::WaitRegistered],
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Wait {
                wake_id: wake("w1"),
                condition: WaitCondition::Deadline {
                    deadline: Timestamp::from_millis(1),
                },
            },
            at: ts(20),
        }),
    );
    let decision = apply(
        Some(&waiting),
        GoalCommand::Wake(Wake {
            goal_id: goal_id(),
            wake_id: wake("w1"),
            next_lease_id: lease("l1"),
            resolution: Some(WaitResolution {
                resolved: WaitCondition::Deadline {
                    deadline: Timestamp::from_millis(1),
                },
                detail: BoundedText::short("deadline passed"),
            }),
            at: ts(30),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    assert!(snapshot.wait_resolution.is_some());
    assert!(
        decision
            .effects
            .contains(&GoalEffect::EmitReminder(GoalReminderKind::WaitResolved))
    );
}

#[test]
fn test_wake_wrong_id_rejected() {
    let running = running_snapshot();
    let waiting = next_snapshot(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: vec![ProgressSignal::WaitRegistered],
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Wait {
                wake_id: wake("w1"),
                condition: WaitCondition::External {
                    description: BoundedText::short("ci"),
                },
            },
            at: ts(20),
        }),
    );
    let err = decide(
        Some(&waiting),
        GoalCommand::Wake(Wake {
            goal_id: goal_id(),
            wake_id: wake("other"),
            next_lease_id: lease("l1"),
            resolution: None,
            at: ts(30),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::WakeNotFound);
}

#[test]
fn test_pause_running_goal_releases_lease() {
    let running = running_snapshot();
    let decision = apply(
        Some(&running),
        GoalCommand::Pause(Pause {
            goal_id: goal_id(),
            reason: PauseReason::UserInterrupt,
            at: ts(30),
        }),
    );
    assert_eq!(
        decision.snapshot.as_ref().unwrap().status(),
        GoalStatus::Paused
    );
    assert!(decision.effects.contains(&GoalEffect::ReleaseLease {
        lease_id: lease("l0")
    }));
}

#[test]
fn test_pause_from_stopped_rejected() {
    let running = running_snapshot();
    let paused = next_snapshot(
        Some(&running),
        GoalCommand::Pause(Pause {
            goal_id: goal_id(),
            reason: PauseReason::UserInterrupt,
            at: ts(30),
        }),
    );
    let err = decide(
        Some(&paused),
        GoalCommand::Pause(Pause {
            goal_id: goal_id(),
            reason: PauseReason::UserInterrupt,
            at: ts(31),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, GoalTransitionError::InvalidTransition { .. }));
}

#[test]
fn test_resume_paused_commits_active_and_resets_streaks() {
    let running = running_snapshot();
    let mut paused = next_snapshot(
        Some(&running),
        GoalCommand::Pause(Pause {
            goal_id: goal_id(),
            reason: PauseReason::UserInterrupt,
            at: ts(30),
        }),
    );
    paused.counters.no_progress_streak = 2;
    paused.counters.unreported_streak = 3;
    let decision = apply(
        Some(&paused),
        GoalCommand::Resume(Resume {
            goal_id: goal_id(),
            next_lease_id: lease("l1"),
            at: ts(40),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    assert_eq!(snapshot.counters.no_progress_streak, 0);
    assert_eq!(snapshot.counters.unreported_streak, 0);
    assert!(decision.effects.contains(&GoalEffect::ScheduleTurn {
        lease_id: lease("l1")
    }));
}

#[test]
fn test_resume_from_budget_limited_requires_budget_raise() {
    let mut created = created_snapshot();
    created.counters.autonomous_turns = created.budget.max_autonomous_turns.get();
    let budget_limited = next_snapshot(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            trigger: GoalTurnTrigger::Autonomous,
            at: ts(10),
        }),
    );
    let err = decide(
        Some(&budget_limited),
        GoalCommand::Resume(Resume {
            goal_id: goal_id(),
            next_lease_id: lease("l1"),
            at: ts(40),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::BudgetRaiseRequired);
}

// ── Edit ─────────────────────────────────────────────────────────────────

#[test]
fn test_edit_spec_mismatch_rejected() {
    let created = created_snapshot();
    let err = decide(
        Some(&created),
        GoalCommand::Edit(Edit {
            goal_id: goal_id(),
            expected_spec_revision: SpecRevision::INITIAL.next(),
            objective: Some(GoalObjective::new("x")),
            contract: None,
            clear_contract: false,
            policy: None,
            budget: None,
            plan_binding: None,
            next_lease_id: None,
            at: ts(10),
        }),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        GoalTransitionError::SpecRevisionMismatch { .. }
    ));
}

#[test]
fn test_edit_objective_bumps_spec_and_emits_reminder() {
    let created = created_snapshot();
    let decision = apply(
        Some(&created),
        GoalCommand::Edit(Edit {
            goal_id: goal_id(),
            expected_spec_revision: SpecRevision::INITIAL,
            objective: Some(GoalObjective::new("new objective")),
            contract: None,
            clear_contract: false,
            policy: None,
            budget: None,
            plan_binding: None,
            next_lease_id: None,
            at: ts(10),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.spec_revision, SpecRevision::INITIAL.next());
    assert_eq!(snapshot.objective.text.as_str(), "new objective");
    assert!(decision.effects.contains(&GoalEffect::EmitReminder(
        GoalReminderKind::ObjectiveChanged
    )));
}

#[test]
fn test_budget_edit_and_resume_from_budget_limited_turns() {
    let mut created = created_snapshot();
    created.counters.autonomous_turns = created.budget.max_autonomous_turns.get();
    let budget_limited = next_snapshot(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            trigger: GoalTurnTrigger::Autonomous,
            at: ts(10),
        }),
    );
    let raised = GoalBudget {
        max_autonomous_turns: std::num::NonZeroU32::new(40).unwrap(),
        ..GoalBudget::default()
    };
    let decision = apply(
        Some(&budget_limited),
        GoalCommand::Edit(Edit {
            goal_id: goal_id(),
            expected_spec_revision: budget_limited.spec_revision,
            objective: None,
            contract: None,
            clear_contract: false,
            policy: None,
            budget: Some(raised),
            plan_binding: None,
            next_lease_id: Some(lease("l1")),
            at: ts(20),
        }),
    );
    let snapshot = decision.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    assert!(decision.effects.contains(&GoalEffect::ScheduleTurn {
        lease_id: lease("l1")
    }));
}

#[test]
fn test_budget_edit_insufficient_rejected() {
    let mut created = created_snapshot();
    created.counters.autonomous_turns = created.budget.max_autonomous_turns.get();
    let budget_limited = next_snapshot(
        Some(&created),
        GoalCommand::StartTurn(StartTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            trigger: GoalTurnTrigger::Autonomous,
            at: ts(10),
        }),
    );
    // Keep the same (already-exhausted) turn budget.
    let err = decide(
        Some(&budget_limited),
        GoalCommand::Edit(Edit {
            goal_id: goal_id(),
            expected_spec_revision: budget_limited.spec_revision,
            objective: None,
            contract: None,
            clear_contract: false,
            policy: None,
            budget: Some(GoalBudget::default()),
            plan_binding: None,
            next_lease_id: Some(lease("l1")),
            at: ts(20),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::InvalidBudgetEdit);
}

// ── Clear / acceptance / identity ────────────────────────────────────────

#[test]
fn test_clear_removes_snapshot_and_audits() {
    let running = running_snapshot();
    let decision = apply(
        Some(&running),
        GoalCommand::Clear(Clear {
            goal_id: goal_id(),
            at: ts(30),
        }),
    );
    assert!(decision.snapshot.is_none());
    assert!(
        decision
            .effects
            .contains(&GoalEffect::RecordAudit(GoalAuditKind::Cleared))
    );
    assert!(decision.effects.contains(&GoalEffect::ReleaseLease {
        lease_id: lease("l0")
    }));
}

#[test]
fn test_accept_and_reject_from_user_acceptance() {
    let running = running_snapshot();
    let waiting = next_snapshot(
        Some(&running),
        GoalCommand::FinishTurn(FinishTurn {
            goal_id: goal_id(),
            lease_id: lease("l0"),
            turn_id: turn("t0"),
            reported: true,
            signals: Vec::new(),
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Wait {
                wake_id: wake("accept"),
                condition: WaitCondition::UserAcceptance,
            },
            at: ts(20),
        }),
    );

    // Reject → back to active with recorded rejection.
    let rejected = apply(
        Some(&waiting),
        GoalCommand::RejectCompletion(RejectCompletion {
            goal_id: goal_id(),
            next_lease_id: lease("l1"),
            rejection: CompletionRejection::new(
                CompletionRejectReason::CoverageIncomplete,
                "needs more",
            ),
            at: ts(30),
        }),
    );
    let rejected_snapshot = rejected.snapshot.as_ref().unwrap();
    assert_eq!(rejected_snapshot.status(), GoalStatus::Active);
    assert!(rejected_snapshot.last_rejection.is_some());

    // Accept → completed via a matching authorization.
    let auth = authorization_for(&waiting, &lease("l0"));
    let accepted = apply(
        Some(&waiting),
        GoalCommand::AcceptCompletion(AcceptCompletion {
            authorization: auth,
            at: ts(31),
        }),
    );
    assert_eq!(
        accepted.snapshot.as_ref().unwrap().status(),
        GoalStatus::Completed
    );
}

#[test]
fn test_stale_goal_id_rejected() {
    let created = created_snapshot();
    let err = decide(
        Some(&created),
        GoalCommand::Pause(Pause {
            goal_id: GoalId::new("some-other-goal"),
            reason: PauseReason::UserInterrupt,
            at: ts(10),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, GoalTransitionError::StaleGoalId { .. }));
}

#[test]
fn test_command_on_absent_goal_rejected() {
    let err = decide(
        None,
        GoalCommand::Pause(Pause {
            goal_id: goal_id(),
            reason: PauseReason::UserInterrupt,
            at: ts(10),
        }),
    )
    .unwrap_err();
    assert_eq!(err, GoalTransitionError::NoCurrentGoal);
}

// ── Liveness invariant ───────────────────────────────────────────────────

#[test]
fn test_state_version_advances_on_every_commit() {
    let created = created_snapshot();
    assert_eq!(created.state_version, StateVersion::INITIAL);
    let started = start(&created, "l0", "t0", GoalTurnTrigger::Creation);
    assert_eq!(started.state_version, StateVersion::INITIAL.next());
}

#[test]
fn test_every_active_snapshot_has_a_lease_and_waiting_has_a_wake() {
    // Sample the reachable transitions and assert the structural invariant that
    // active carries a lease and waiting carries a wake (§7.6).
    let created = created_snapshot();
    let running = running_snapshot();
    let continued = finish_continue(
        &running,
        "l0",
        "t0",
        "l1",
        vec![ProgressSignal::ToolObservation],
    )
    .snapshot
    .unwrap();
    for snapshot in [&created, &running, &continued] {
        match &snapshot.lifecycle {
            GoalLifecycle::Active { lease } => {
                let _ = lease.lease_id();
            }
            GoalLifecycle::Waiting { wake } => {
                let _ = &wake.wake_id;
            }
            _ => {}
        }
        assert!(
            snapshot.has_continuation_owner()
                || snapshot.status().is_stopped()
                || snapshot.is_terminal()
        );
    }
}
