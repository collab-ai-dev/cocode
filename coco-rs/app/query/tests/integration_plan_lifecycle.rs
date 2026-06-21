//! End-to-end plan-mode lifecycle integration tests.
//!
//! Exercises the full agent loop (QueryEngine + ToolExecutor +
//! PlanModeReminder + Enter/ExitPlanMode tools) against a scripted mock
//! LLM, with plan-mode cadence observed through a shared
//! `Arc<RwLock<ToolAppState>>` across multiple `engine.run*` calls.
//!
//! Unit tests live next to their owners (reminder, tools, permissions);
//! these tests catch regressions that only manifest when the pieces are
//! wired together — specifically:
//!
//! - Fix B: human-turn cadence — tool-result rounds inside a single
//!   human turn must NOT advance
//!   `plan_mode_turns_since_last_attachment`. The unit test for
//!   `PlanModeReminder` exercises this at the reminder level; this file
//!   exercises it at the engine level, with a real multi-tool-round
//!   run.
//!
//! - Reentry coupling — `## Re-entering Plan Mode` must co-emit with a
//!   fresh Full reminder on the turn after a prior `ExitPlanMode`.
//!   Previously these were mutually exclusive.
//!
//! - Attachment cadence persistence — the throttle counter lives on
//!   `ToolAppState` so it must survive across `engine.run*` calls
//!   (each call spawns a fresh `PlanModeReminder`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "mock_harness.rs"]
mod harness;

use std::sync::Arc;

use coco_messages::Message;
use coco_types::CoreEvent;
use coco_types::PermissionMode;
use coco_types::ServerNotification;
use coco_types::ToolAppState;
use harness::MockModelBuilder;
use harness::MockResponse;
use harness::PlanModeTurnParams;
use harness::run_plan_mode_turn;
use harness::run_plan_mode_turn_with_events;
use harness::tools_with_plan_mode;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::RwLock;

/// Pull the plan-file path the engine will resolve for a given
/// config_home + session_id. Shared by tests that need to pre-write a
/// plan to disk (so `ExitPlanMode` reads the content, because our mock
/// doesn't actually edit files via the Write tool — it just calls
/// `ExitPlanMode` and expects the content to be there).
fn plan_file_path(config_home: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let plans_dir = coco_context::resolve_plans_directory(config_home, None, None);
    coco_context::get_plan_file_path(session_id, &plans_dir, None)
}

/// Count `Message::Attachment` whose rendered text contains `needle`.
/// Cheap substring match is enough: each plan-mode banner has a
/// distinctive top-level `## ...` heading.
fn count_attachments_containing<M: std::borrow::Borrow<Message>>(
    messages: &[M],
    needle: &str,
) -> usize {
    messages
        .iter()
        .filter_map(|m| match m.borrow() {
            Message::Attachment(a) => match a.as_api_message() {
                Some(coco_messages::LlmMessage::User { content, .. }) => {
                    content.iter().find_map(|c| match c {
                        coco_messages::UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .filter(|text| text.contains(needle))
        .count()
}

/// Count `plan_mode` reminder attachments in the transcript. Cadence is now
/// history-derived, so the transcript — not an app_state counter — is the
/// source of truth for "how many plan-mode reminders fired".
fn count_plan_mode_attachments<M: std::borrow::Borrow<Message>>(messages: &[M]) -> usize {
    messages
        .iter()
        .filter(|m| {
            matches!(
                m.borrow(),
                Message::Attachment(a) if a.kind == coco_types::AttachmentKind::PlanMode
            )
        })
        .count()
}

fn attachment_texts<M: std::borrow::Borrow<Message>>(messages: &[M]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m.borrow() {
            Message::Attachment(a) => match a.as_api_message() {
                Some(coco_messages::LlmMessage::User { content, .. }) => {
                    content.iter().find_map(|c| match c {
                        coco_messages::UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn permission_mode_changes(events: &[CoreEvent]) -> Vec<PermissionMode> {
    events
        .iter()
        .filter_map(|event| match event {
            CoreEvent::Protocol(ServerNotification::PermissionModeChanged(params)) => {
                Some(params.mode)
            }
            _ => None,
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────
// Test 1: end-to-end Plan → Exit single run
// ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn end_to_end_single_run() {
    // Scripted flow (starting in Plan mode):
    //   call 0: ExitPlanMode tool call
    //   call 1: "done" text
    //
    // Assert: plan-mode state flags propagate through app_state as
    // expected after the single run.
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-single-run";
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    // Pre-seed the plan file so ExitPlanMode has content to return.
    // A real session would have the model edit the plan via the Write
    // tool before calling Exit; we skip that here to keep the
    // scripted flow small — the fix we're guarding against is about
    // the reminder/state, not the plan-editing path.
    let path = plan_file_path(tmp.path(), session_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# test plan\n").unwrap();

    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::tool_call("ExitPlanMode", json!({})))
        .on_call(1, |_| MockResponse::text("done"))
        .build();

    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "plan a small refactor",
    );
    let result = run_plan_mode_turn(model, params).await;

    // Model finished with "done".
    assert_eq!(result.response_text, "done");

    // Observable evidence: within this single run, the exit banner
    // must have been emitted to history — the reminder consumes the
    // `needs_plan_mode_exit_attachment` flag at the next turn_start
    // after ExitPlanMode runs.
    let exit_banner_hits =
        count_attachments_containing(&result.final_messages, "## Exited Plan Mode");
    assert!(
        exit_banner_hits >= 1,
        "ExitPlanMode must fire the exit banner in the next turn_start; \
         got {exit_banner_hits} exit banners in final_messages"
    );

    // Evidence the plan reminder fired on turn 1: the Full reminder's
    // distinctive workflow heading must appear in final_messages.
    // (We can't use `plan_mode_attachment_count` because the exit
    // banner on turn 2 resets the cadence counter to 0 — TS parity:
    // `countPlanModeAttachmentsSinceLastExit` stops counting at exits.)
    let full_hits = count_attachments_containing(&result.final_messages, "## Plan Workflow");
    assert!(
        full_hits >= 1,
        "turn 1 must emit a Full plan reminder; got {full_hits} in \
         final_messages"
    );

    // ExitPlanMode's live-mode write: app_state should now reflect
    // Default (the restore target when pre_plan_mode was None).
    // TS parity: ExitPlanModeV2Tool.ts:357-403 flips
    // `appState.toolPermissionContext.mode` to restoreMode.
    let guard = app_state.read().await;
    assert_eq!(
        guard.permissions.mode,
        Some(PermissionMode::Default),
        "ExitPlanMode.execute must write restore_mode to \
         app_state.permission_mode — TS parity with setAppState"
    );
}

#[tokio::test]
async fn no_plan_exit_does_not_reference_stale_plan_file() {
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-no-plan-exit";
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    let path = plan_file_path(tmp.path(), session_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# stale implementation plan\n").unwrap();

    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::tool_call("ExitPlanMode", json!({})))
        .on_call(1, |_| MockResponse::text("answered"))
        .build();

    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "explain where routing lives",
    );
    let result = run_plan_mode_turn(model, params).await;

    assert_eq!(result.response_text, "answered");
    let exit_text = attachment_texts(&result.final_messages)
        .into_iter()
        .find(|text| text.contains("## Exited Plan Mode"))
        .expect("exit banner");
    assert!(
        exit_text.contains("without an implementation plan"),
        "{exit_text}"
    );
    assert!(
        !exit_text.contains(path.to_string_lossy().as_ref()),
        "no-plan exit must not reference stale plan file: {exit_text}"
    );
    let guard = app_state.read().await;
    assert_eq!(
        guard.pending_plan_mode_exit_outcome, None,
        "exit reminder should consume the pending no-plan outcome"
    );
}

// ─────────────────────────────────────────────────────────────────
// Test 2: tool-result rounds inside a single human turn do NOT
// advance the cadence counter (regression guard for fix B)
// ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn tool_rounds_do_not_advance_cadence_within_single_human_turn() {
    // One human turn, many tool rounds:
    //   Read → Read → Read → Read → Read → text
    //
    // All 5 Reads are separate LLM iterations (the engine's internal
    // `turn` counter ticks for each), but they all share the one user
    // message's UUID — so the reminder's `observe_turn_and_count`
    // must refuse to bump. Before fix B, the counter bumped on every
    // `turn_start` call, which would have triggered a premature
    // Sparse reminder on iteration 5.
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("scratch.txt");
    std::fs::write(&file_path, "scratch").unwrap();
    let read_target = file_path.to_string_lossy().into_owned();

    let session_id = "integ-tool-rounds";
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    let r1 = read_target.clone();
    let r2 = read_target.clone();
    let r3 = read_target.clone();
    let r4 = read_target.clone();
    let r5 = read_target.clone();
    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::tool_call("Read", json!({"file_path": r1.clone()}))
        })
        .on_call(1, move |_| {
            MockResponse::tool_call("Read", json!({"file_path": r2.clone()}))
        })
        .on_call(2, move |_| {
            MockResponse::tool_call("Read", json!({"file_path": r3.clone()}))
        })
        .on_call(3, move |_| {
            MockResponse::tool_call("Read", json!({"file_path": r4.clone()}))
        })
        .on_call(4, move |_| {
            MockResponse::tool_call("Read", json!({"file_path": r5.clone()}))
        })
        .on_call(5, |_| MockResponse::text("explored"))
        .build();

    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "explore the module",
    );
    let result = run_plan_mode_turn(model, params).await;
    assert_eq!(result.response_text, "explored");

    // TS parity: across 5 Read iterations within one human turn, only the
    // FIRST plan-mode attachment fired. History-derived cadence sees no new
    // human-turn user message across the tool-result rounds, so
    // `human_turns_since_attachment_opt(PlanMode)` stays `Some(0)` and blocks
    // re-firing — no Sparse reminder on iterations 2-5.
    let plan_attachments = count_plan_mode_attachments(&result.final_messages);
    assert_eq!(
        plan_attachments, 1,
        "tool-result rounds must not re-fire the plan-mode reminder; got {plan_attachments}"
    );
}

// ─────────────────────────────────────────────────────────────────
// Test 3: cadence across six human turns — attachment #1 (Full) on
// turn 1, #2 (Sparse) on turn 6, none in between
// ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cadence_across_six_human_turns() {
    // Drives six separate `run_*` calls, each representing one human
    // turn. History accumulates across runs so the reminder sees a
    // new non-meta user UUID each time (the newly pushed user message)
    // and correctly bumps the throttle.
    //
    // Expected reminder attachments (with engine starting in Plan):
    //   Turn 1 → #1 Full (first plan-mode turn; `attachment_count == 0`)
    //   Turns 2-5 → throttled
    //   Turn 6 → #2 Sparse
    //
    // Each mock response is a single text reply so each run executes
    // exactly one LLM iteration — keeping the engine's internal turn
    // counter aligned with the human-turn counter for a clean test.
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-cadence-6";
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    let plan_path = plan_file_path(tmp.path(), session_id);
    std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();

    let tools = tools_with_plan_mode();

    // Each turn gets a fresh mock because the mock's call counter is
    // stateful — reusing across runs would misalign scripted responses.
    let make_model = |text: &'static str| {
        MockModelBuilder::new()
            .on_call(0, move |_| MockResponse::text(text))
            .build()
    };

    // Turn 1 — from scratch.
    let turn1 = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools.clone(),
        "start planning",
    );
    let r1 = run_plan_mode_turn(make_model("stage 1 done"), turn1).await;
    assert_eq!(r1.response_text, "stage 1 done");
    let count_after_1 = count_plan_mode_attachments(&r1.final_messages);
    assert_eq!(count_after_1, 1, "turn 1 must emit attachment #1");

    // Turns 2..=5 — each appends a new user message on top of the
    // accumulated history. Throttled: no new attachments.
    let mut messages = r1.final_messages;
    for (turn_no, prompt, reply) in [
        (2, "keep going", "stage 2 done"),
        (3, "more", "stage 3 done"),
        (4, "next", "stage 4 done"),
        (5, "yet more", "stage 5 done"),
    ] {
        let params = PlanModeTurnParams::plan_turn(
            session_id,
            tmp.path().to_path_buf(),
            app_state.clone(),
            tools.clone(),
            "unused",
        )
        .next_turn(messages, prompt);
        let r = run_plan_mode_turn(make_model(reply), params).await;
        messages = r.final_messages;
        let count = count_plan_mode_attachments(&messages);
        assert_eq!(
            count, 1,
            "turn {turn_no} must NOT emit a new attachment (throttled)"
        );
    }

    // Turn 6 — cadence fires. With TURNS_BETWEEN_ATTACHMENTS=5 and
    // FULL_REMINDER_EVERY_N_ATTACHMENTS=5, attachment #2 is Sparse.
    let turn6 = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools.clone(),
        "unused",
    )
    .next_turn(messages, "final push");
    let r6 = run_plan_mode_turn(make_model("stage 6 done"), turn6).await;

    let count_after_6 = count_plan_mode_attachments(&r6.final_messages);
    assert_eq!(
        count_after_6, 2,
        "turn 6 must emit attachment #2 (Sparse); got count={count_after_6}"
    );
    // Sparse variant has a distinctive opener that Full doesn't use;
    // verify the second banner in the final history is Sparse.
    let sparse_hits = count_attachments_containing(&r6.final_messages, "Plan mode still active");
    assert!(
        sparse_hits >= 1,
        "turn 6's emitted reminder should be the Sparse variant"
    );
}

// ─────────────────────────────────────────────────────────────────
// Test 4: reentry co-emits Reentry banner + fresh Full reminder
// ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reentry_co_emits_with_full_on_next_plan_entry() {
    // Reentry scenario: a prior session in Plan mode ended with the
    // user exiting, and they're now re-entering Plan with a plan file
    // already on disk. The reminder's turn_start must emit BOTH the
    // Reentry banner AND a fresh Full reminder on this first turn back
    // in Plan — they're co-emitted, not mutually exclusive (fix #1
    // from the earlier pass).
    //
    // This test simulates the cross-run state by pre-populating
    // `app_state.has_exited_plan_mode = true` before the run. In real
    // life this flag is set by `ExitPlanMode.execute` on a prior run;
    // cross-run persistence is one of `ToolAppState`'s jobs. We don't
    // call `ExitPlanMode` here because the engine's permission_mode
    // is snapshotted at construction (see the architecture note on
    // `end_to_end_single_run`), so a prior-run Exit would consume the
    // has_exited flag within the same run via Reentry logic —
    // defeating the cross-run nature of the test.
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-reentry";

    let plan_path = plan_file_path(tmp.path(), session_id);
    std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
    // Plan content must be on disk for the reentry check
    // (`plan_exists` gate on Reentry emission in `plan_mode_reminder.rs`).
    std::fs::write(&plan_path, "# plan from previous session\n").unwrap();

    // Seed app_state as if a previous run had called ExitPlanMode.
    let app_state = Arc::new(RwLock::new(ToolAppState {
        has_exited_plan_mode: true,
        ..Default::default()
    }));

    // Re-enter Plan mode — user Shift+Tabbed back in.
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text("re-planning"))
        .build();

    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "replan with a new direction",
    )
    .with_permission_mode(PermissionMode::Plan);
    let result = run_plan_mode_turn(model, params).await;
    assert_eq!(result.response_text, "re-planning");

    // Co-emission check: the history must contain BOTH a Reentry
    // banner AND a Full reminder (Plan Workflow heading).
    let reentry_hits =
        count_attachments_containing(&result.final_messages, "## Re-entering Plan Mode");
    let full_hits = count_attachments_containing(&result.final_messages, "## Plan Workflow");
    assert!(
        reentry_hits >= 1,
        "must emit a Reentry banner; got {reentry_hits} Reentry messages \
         in final history"
    );
    assert!(
        full_hits >= 1,
        "must ALSO emit a Full reminder alongside Reentry; got \
         {full_hits} Full reminders"
    );

    // The has_exited flag must be cleared by the reminder after Reentry.
    assert!(
        !app_state.read().await.has_exited_plan_mode,
        "has_exited_plan_mode must be cleared after Reentry fires"
    );
}

// ─────────────────────────────────────────────────────────────────
// Test 5: model-driven EnterPlanMode mid-run flips reminder ON
// (regression guard for Bug 2 — frozen config.permission_mode)
// ─────────────────────────────────────────────────────────────────
//
// Before the app_state migration, `config.permission_mode` was
// snapshotted at engine construction and `PlanModeReminder::new`
// captured it as a plain field. If the model called EnterPlanMode
// mid-run, `ctx.permission_context.mode` flipped but the reminder's
// frozen value didn't — so subsequent turns saw "Default" and never
// emitted a plan reminder. After the fix, `EnterPlanMode::execute`
// writes `app_state.permission_mode = Plan`, the reminder reads it
// live on the next turn_start, and the plan-mode banner fires.
//
// TS parity: ExitPlanModeV2Tool.ts:88-94 sets mode via setAppState;
// every subsequent `getAppState()` sees Plan. Rust now matches.

#[tokio::test]
async fn model_driven_enter_plan_mode_flips_reminder_on_next_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-model-enter";
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    // Script:
    //   call 0: EnterPlanMode tool call
    //   call 1: text "exploring" (first LLM response after tool — same
    //           turn, but turn_start will have re-observed app_state)
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::tool_call("EnterPlanMode", json!({})))
        .on_call(1, |_| MockResponse::text("exploring"))
        .build();

    // Engine starts in Default. Model calls EnterPlanMode → app_state
    // flips to Plan. Next turn_start emits the plan reminder.
    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "let's plan",
    )
    .with_permission_mode(PermissionMode::Default);
    let (result, events) = run_plan_mode_turn_with_events(model, params).await;
    assert_eq!(result.response_text, "exploring");
    assert_eq!(permission_mode_changes(&events), vec![PermissionMode::Plan]);

    // Evidence: the Full plan reminder (with its workflow heading)
    // must be in final_messages. Before the fix, this would be 0.
    let full_hits = count_attachments_containing(&result.final_messages, "## Plan Workflow");
    assert!(
        full_hits >= 1,
        "after model-driven EnterPlanMode, the reminder must fire on \
         the next turn_start; got {full_hits} Full reminders in history"
    );

    // app_state reflects the tool's mode transition.
    let guard = app_state.read().await;
    assert_eq!(
        guard.permissions.mode,
        Some(PermissionMode::Plan),
        "EnterPlanMode.execute must flip app_state.permission_mode to Plan"
    );
    assert_eq!(
        guard.permissions.pre_plan_mode,
        Some(PermissionMode::Default),
        "prior mode must be stashed for later ExitPlanMode to restore"
    );
}

#[tokio::test]
async fn model_driven_enter_plan_mode_swaps_to_plan_role_client_same_run() {
    // The engine can enter plan mode via a model tool call inside the
    // same run. The next LLM iteration must observe live app_state and
    // route through the Plan-role client, not the Main client captured
    // at engine construction.
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-model-enter-plan-client";
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    let main_model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::tool_call("EnterPlanMode", json!({})))
        .on_call(1, |_| MockResponse::text("main-client"))
        .build();
    let plan_model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::text("plan-client"))
        .build();

    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "switch to planning",
    )
    .with_permission_mode(PermissionMode::Default)
    .with_plan_role_model(plan_model);
    let result = run_plan_mode_turn(main_model, params).await;

    assert_eq!(
        result.response_text, "plan-client",
        "post-EnterPlanMode iteration must use the Plan-role client"
    );
    assert_eq!(
        app_state.read().await.permissions.mode,
        Some(PermissionMode::Plan),
    );
}

// ─────────────────────────────────────────────────────────────────
// Test 6: model-driven ExitPlanMode stops plan reminder on next turn
// (regression guard for Bug 2 — the reverse direction)
// ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn model_driven_exit_plan_mode_stops_reminder_on_next_turn() {
    // Engine starts in Plan. Model calls ExitPlanMode. On the next
    // turn_start the reminder should see app_state.permission_mode =
    // Default (written by ExitPlanMode.execute) and NOT emit a plan
    // reminder — only the one-shot exit banner. Before the fix, the
    // reminder kept firing plan reminders because its frozen mode
    // stayed Plan.
    let tmp = tempfile::tempdir().unwrap();
    let session_id = "integ-model-exit";
    let plan_path = plan_file_path(tmp.path(), session_id);
    std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
    std::fs::write(&plan_path, "# plan\n").unwrap();
    let app_state = Arc::new(RwLock::new(ToolAppState::default()));

    // Script: ExitPlanMode on turn 1 (inside a same-run multi-iteration
    // loop, turn_start runs multiple times so we can observe the
    // plan-reminder-stops behavior).
    let model = MockModelBuilder::new()
        .on_call(0, |_| MockResponse::tool_call("ExitPlanMode", json!({})))
        .on_call(1, |_| {
            MockResponse::tool_call("Read", json!({"file_path": "/dev/null"}))
        })
        .on_call(2, |_| MockResponse::text("done"))
        .build();

    let params = PlanModeTurnParams::plan_turn(
        session_id,
        tmp.path().to_path_buf(),
        app_state.clone(),
        tools_with_plan_mode(),
        "finish and code",
    );
    let (result, events) = run_plan_mode_turn_with_events(model, params).await;
    assert_eq!(result.response_text, "done");
    assert_eq!(
        permission_mode_changes(&events),
        vec![PermissionMode::Default]
    );

    // Exit banner must have fired ONCE (one-shot semantics).
    let exit_banner_hits =
        count_attachments_containing(&result.final_messages, "## Exited Plan Mode");
    assert_eq!(
        exit_banner_hits, 1,
        "exit banner must fire exactly once; got {exit_banner_hits}"
    );

    // Count plan reminders emitted AFTER the exit banner. There should
    // be none — the reminder reads app_state.permission_mode = Default
    // and skips dispatch. Before the fix, the plan reminder would keep
    // firing because its frozen mode stayed Plan.
    //
    // Easiest check: total plan reminders across the whole history
    // should be exactly 1 (the initial Full on turn 1 before
    // ExitPlanMode). Anything more means the reminder fired after exit.
    let plan_reminder_hits =
        count_attachments_containing(&result.final_messages, "## Plan Workflow");
    assert_eq!(
        plan_reminder_hits, 1,
        "plan reminder should fire exactly once (turn 1, before \
         ExitPlanMode). After the exit, the reminder must read the \
         live Default mode from app_state and stop firing — got \
         {plan_reminder_hits} plan reminders total"
    );

    // app_state reflects the live mode change.
    assert_eq!(
        app_state.read().await.permissions.mode,
        Some(PermissionMode::Default),
    );
}
