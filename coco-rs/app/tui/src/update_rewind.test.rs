use crate::state::AppState;
use crate::state::rewind::RestoreType;
use crate::state::rewind::RewindPhase;
use crate::transcript::derive::test_helpers;

use super::*;

/// Map a legacy test id (`"msg-1"`) to the v5 UUID the cell-push helper
/// would have produced for the same id. Returns a `Uuid` (not a String)
/// so callers can pass it directly to `build_rewind_state_for_uuid` and
/// compare against `RewindableMessage.message_id: Uuid`.
fn test_uuid(s: &str) -> uuid::Uuid {
    crate::transcript::derive::id_to_uuid(s)
}

fn make_state_with_messages(count: i32) -> AppState {
    let mut state = AppState::new();
    for i in 0..count {
        test_helpers::push_user_text(
            &mut state.session,
            &format!("msg-{i}"),
            &format!("Hello turn {i}"),
        );
        test_helpers::push_assistant_text(&mut state.session, "Hi there");
    }
    state
}

/// Helper: extract dispatch tuple from a `ConfirmOutcome::Dispatch`,
/// panicking with a message on any other variant.
fn expect_dispatch(outcome: ConfirmOutcome, ctx: &str) -> (String, RestoreType) {
    match outcome {
        ConfirmOutcome::Dispatch {
            message_id,
            restore,
        } => (message_id, restore),
        other => panic!("{ctx}: expected Dispatch, got {other:?}"),
    }
}

fn set_selected_diff_stats(state: &mut RewindState, files_changed: i32) {
    let selected = state.selected as usize;
    let stats = crate::state::DiffStatsPreview {
        insertions: i64::from(files_changed),
        deletions: 0,
        file_paths: (0..files_changed).map(|i| format!("file-{i}.rs")).collect(),
    };
    state.messages[selected].diff_stats = Some(stats.clone());
    state.messages[selected].can_restore_code = Some(true);
    state.diff_stats = Some(stats);
    state.diff_stats_message_id = Some(state.messages[selected].message_id);
}

#[test]
fn test_build_rewind_state_appends_synthetic_current_row() {
    // A virtual `(current)` row is appended at the end of the message list.
    // After the synthetic-row landing there are N+1 entries: N real + 1
    // synthetic, with selection on the synthetic row.
    let state = make_state_with_messages(3);
    let state = build_rewind_state(&state);

    assert_eq!(state.messages.len(), 4);
    let last = state.messages.last().unwrap();
    assert!(last.is_current_prompt);
    assert_eq!(
        last.message_id,
        uuid::Uuid::nil(),
        "synthetic row uses Uuid::nil() sentinel — gate is the is_current_prompt flag",
    );
    assert_eq!(state.selected, 3);
    assert_eq!(state.phase, RewindPhase::MessageSelect);
    // The last *real* message is at index N-1 = 2.
    assert_eq!(state.messages[2].message_id, test_uuid("msg-2"));
    assert!(!state.messages[2].is_current_prompt);
}

#[test]
fn test_build_rewind_state_does_not_populate_row_stats_eagerly() {
    // Per-row `+X -Y` is loaded asynchronously via
    // `TuiOnlyEvent::RewindRowMetadataReady`. The picker open path
    // must NOT shove stub stats onto rows — the renderer keys "still
    // loading" off `can_restore_code == None`.
    let mut state = AppState::new();
    state.session.file_history_enabled = true;
    test_helpers::push_user_text(&mut state.session, "u1", "first");
    test_helpers::push_user_text(&mut state.session, "u2", "second");

    let rewind = build_rewind_state(&state);

    assert_eq!(rewind.messages.len(), 3, "two real rows + synthetic");
    for row in rewind.messages.iter().filter(|m| !m.is_current_prompt) {
        assert!(row.diff_stats.is_none(), "row stats arrive async");
        assert!(
            row.can_restore_code.is_none(),
            "can_restore unknown until RewindRowMetadataReady arrives"
        );
    }
}

#[test]
fn test_handle_rewind_nav() {
    let state = make_state_with_messages(5);
    let mut state = build_rewind_state(&state);
    // Synthetic current-prompt row at index 5 (5 real + synthetic).
    assert_eq!(state.selected, 5);

    handle_rewind_nav(&mut state, -1);
    assert_eq!(state.selected, 4); // last real

    handle_rewind_nav(&mut state, -10);
    assert_eq!(state.selected, 0); // first real

    handle_rewind_nav(&mut state, 100);
    assert_eq!(state.selected, 5); // back to synthetic
}

#[test]
fn test_handle_rewind_nav_jump_to_top_and_bottom() {
    let state = make_state_with_messages(4);
    let mut state = build_rewind_state(&state);

    handle_rewind_nav(&mut state, i32::MIN / 2);
    assert_eq!(state.selected, 0);

    handle_rewind_nav(&mut state, i32::MAX / 2);
    assert_eq!(state.selected, 4);
}

#[test]
fn test_handle_rewind_confirm_synthetic_row_dismisses() {
    // Confirm on the virtual current-prompt row never dispatches; it
    // closes the state (equivalent to Esc).
    let state = make_state_with_messages(3);
    let mut state = build_rewind_state(&state);
    // Default selection lands on the synthetic row.
    let outcome = handle_rewind_confirm(&mut state);
    assert_eq!(outcome, ConfirmOutcome::Dismiss);
}

#[test]
fn test_handle_rewind_confirm_transitions_to_options() {
    let state = make_state_with_messages(3);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;

    // Move off the synthetic row to the last real message first.
    handle_rewind_nav(&mut state, -1);
    set_selected_diff_stats(&mut state, 1);
    assert!(!state.messages[state.selected as usize].is_current_prompt);

    // Confirm in MessageSelect -> transitions to RestoreOptions when
    // file history is enabled (TS path that loads diffStatsForRestore).
    let outcome = handle_rewind_confirm(&mut state);
    assert_eq!(outcome, ConfirmOutcome::Phase);
    assert_eq!(state.phase, RewindPhase::RestoreOptions);
    assert!(!state.available_options.is_empty());
}

#[test]
fn test_handle_rewind_confirm_requests_diff_stats_before_options() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1);

    let outcome = handle_rewind_confirm(&mut state);
    assert_eq!(
        outcome,
        ConfirmOutcome::RequestDiffStats {
            message_id: test_uuid("msg-1").to_string(),
        }
    );
    assert_eq!(state.phase, RewindPhase::MessageSelect);
    assert!(state.available_options.is_empty());
}

#[test]
fn test_handle_rewind_confirm_no_code_restore_opens_options_without_code_choices() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1);
    let selected = state.selected as usize;
    state.messages[selected].can_restore_code = Some(false);

    let outcome = handle_rewind_confirm(&mut state);

    assert_eq!(outcome, ConfirmOutcome::Phase);
    assert_eq!(state.phase, RewindPhase::RestoreOptions);
    assert_eq!(
        state.available_options,
        vec![
            RestoreType::ConversationOnly,
            RestoreType::SummarizeFrom { feedback: None },
            RestoreType::Nevermind,
        ]
    );
}

#[test]
fn test_handle_rewind_confirm_file_history_off_dispatches_directly() {
    // When file history is disabled the picker bypasses the option screen
    // and immediately dispatches a conversation rewind.
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = false;
    handle_rewind_nav(&mut state, -1); // off the synthetic row
    let outcome = handle_rewind_confirm(&mut state);
    let (msg_id, restore_type) = expect_dispatch(outcome, "file_history_off");
    assert_eq!(msg_id, test_uuid("msg-1").to_string());
    assert_eq!(restore_type, RestoreType::ConversationOnly);
    assert_eq!(state.phase, RewindPhase::MessageSelect);
}

#[test]
fn test_handle_rewind_confirm_returns_selection() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1); // off synthetic
    set_selected_diff_stats(&mut state, 1);

    // Transition to RestoreOptions
    let phase_outcome = handle_rewind_confirm(&mut state);
    assert_eq!(phase_outcome, ConfirmOutcome::Phase);
    assert_eq!(state.phase, RewindPhase::RestoreOptions);

    // Confirm first option (Both)
    let outcome = handle_rewind_confirm(&mut state);
    let (msg_id, restore_type) = expect_dispatch(outcome, "first option");
    assert_eq!(msg_id, test_uuid("msg-1").to_string());
    assert_eq!(restore_type, RestoreType::Both);
}

#[test]
fn test_handle_rewind_confirm_nevermind_returns_to_message_select() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1);
    set_selected_diff_stats(&mut state, 1);

    handle_rewind_confirm(&mut state); // -> RestoreOptions
    assert_eq!(state.phase, RewindPhase::RestoreOptions);

    // Move selection to the Nevermind option (always last).
    state.option_selected = (state.available_options.len() as i32) - 1;
    assert_eq!(
        state.available_options[state.option_selected as usize],
        RestoreType::Nevermind
    );

    let outcome = handle_rewind_confirm(&mut state);
    assert_eq!(outcome, ConfirmOutcome::Phase, "Nevermind never dispatches");
    assert_eq!(
        state.phase,
        RewindPhase::MessageSelect,
        "Nevermind returns to message picker"
    );
    assert!(state.available_options.is_empty());
}

#[test]
fn test_handle_rewind_cancel_dismiss_from_message_select() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);

    assert!(handle_rewind_cancel(&mut state));
}

#[test]
fn test_handle_rewind_cancel_goes_back_from_options() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1);
    set_selected_diff_stats(&mut state, 0);

    handle_rewind_confirm(&mut state); // -> RestoreOptions
    assert_eq!(state.phase, RewindPhase::RestoreOptions);

    assert!(!handle_rewind_cancel(&mut state)); // -> back to MessageSelect
    assert_eq!(state.phase, RewindPhase::MessageSelect);
}

#[test]
fn test_summarize_feedback_empty_submit_cancels_to_options() {
    // Empty submit must NOT dispatch a rewind — it returns to the option
    // list with feedback cleared.
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1);
    set_selected_diff_stats(&mut state, 0);

    handle_rewind_confirm(&mut state); // -> RestoreOptions
    state.option_selected = state
        .available_options
        .iter()
        .position(|o| matches!(o, RestoreType::SummarizeFrom { .. }))
        .expect("SummarizeFrom must be offered") as i32;
    handle_rewind_confirm(&mut state); // -> SummarizeFeedback
    assert_eq!(state.phase, RewindPhase::SummarizeFeedback);
    assert!(state.pending_summarize.is_some());

    // Empty submit
    state.summarize_feedback.clear();
    let outcome = handle_rewind_confirm(&mut state);
    assert_eq!(outcome, ConfirmOutcome::Phase);
    assert_eq!(state.phase, RewindPhase::RestoreOptions);
    assert!(state.pending_summarize.is_none());
    assert!(state.summarize_feedback.is_empty());
}

#[test]
fn test_summarize_feedback_with_text_dispatches_rewind() {
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state(&state);
    state.file_history_enabled = true;
    handle_rewind_nav(&mut state, -1);
    set_selected_diff_stats(&mut state, 0);

    handle_rewind_confirm(&mut state);
    state.option_selected = state
        .available_options
        .iter()
        .position(|o| matches!(o, RestoreType::SummarizeFrom { .. }))
        .expect("SummarizeFrom must be offered") as i32;
    handle_rewind_confirm(&mut state);
    assert_eq!(state.phase, RewindPhase::SummarizeFeedback);

    state.summarize_feedback = "trim me  ".to_string();
    let outcome = handle_rewind_confirm(&mut state);
    let (msg_id, restore) = expect_dispatch(outcome, "summarize feedback");
    assert_eq!(msg_id, test_uuid("msg-1").to_string());
    match restore {
        RestoreType::SummarizeFrom { feedback } => {
            assert_eq!(feedback.as_deref(), Some("trim me"));
        }
        other => panic!("expected SummarizeFrom, got {other:?}"),
    }
    // pending_summarize stays set so Confirming phase can show
    // "Summarizing…" while the engine processes the request.
    assert!(state.pending_summarize.is_some());
}

#[test]
fn test_strip_ide_context_tags_removes_blocks() {
    let input = "Hello <ide_opened_file path='a'>file body</ide_opened_file>\nworld";
    assert_eq!(strip_ide_context_tags(input), "Hello \nworld");
    let nested = "<ide_selection>x</ide_selection>only text";
    assert_eq!(strip_ide_context_tags(nested), "only text");
    let plain = "no tags here";
    assert_eq!(strip_ide_context_tags(plain), "no tags here");
}

#[test]
fn test_strip_prompt_xml_tags_drops_known_blocks() {
    let input = "<commit_analysis>x</commit_analysis>real content";
    assert_eq!(strip_prompt_xml_tags(input), "real content");
    let preserved = "<other_tag>kept</other_tag>";
    assert_eq!(strip_prompt_xml_tags(preserved), preserved);
}

#[test]
fn test_display_text_for_rewind_row_matches_ts_special_cases() {
    assert_eq!(
        display_text_for_rewind_row("<commit_analysis>x</commit_analysis>"),
        crate::i18n::t!("dialog.rewind_empty_message").to_string()
    );
    assert_eq!(
        display_text_for_rewind_row("<bash-input>git status</bash-input>"),
        "! git status"
    );
    assert_eq!(
        display_text_for_rewind_row(
            "<command-message>review</command-message><command-args>--fast</command-args>",
        ),
        "/review --fast"
    );
    assert_eq!(
        display_text_for_rewind_row(
            "<command-message>lint</command-message><skill-format>true</skill-format>",
        ),
        "Skill(lint)"
    );
    assert_eq!(display_text_for_rewind_row("plain prompt"), "plain prompt");
}

// ── preselectedMessage flow via build_rewind_state_for_uuid ─────────

#[test]
fn test_build_rewind_state_for_uuid_jumps_to_options_phase() {
    // When a target UUID is provided, the picker skips the message-select
    // phase entirely and lands on the confirm screen with that message selected.
    let state = make_state_with_messages(3);
    let target = test_uuid("msg-1");
    let state = build_rewind_state_for_uuid(&state, target);
    assert_eq!(state.phase, RewindPhase::RestoreOptions);
    assert!(state.preselected);
    let row = &state.messages[state.selected as usize];
    assert_eq!(row.message_id, target);
    assert!(!row.is_current_prompt);
    assert!(!state.available_options.is_empty());
}

#[test]
fn test_build_rewind_state_for_uuid_unknown_falls_back_to_pick_list() {
    let state = make_state_with_messages(2);
    let unknown = uuid::Uuid::new_v4();
    let state = build_rewind_state_for_uuid(&state, unknown);
    assert_eq!(state.phase, RewindPhase::MessageSelect);
    assert!(
        !state.preselected,
        "miss must leave preselected=false and fall back to the picker"
    );
}

#[test]
fn test_build_rewind_state_for_uuid_nil_falls_back_to_pick_list() {
    // `Uuid::nil()` is the synthetic-row sentinel; passing it MUST NOT
    // resolve to the synthetic row (the `is_current_prompt` filter is
    // explicit in `build_rewind_state_for_uuid`).
    let state = make_state_with_messages(2);
    let state = build_rewind_state_for_uuid(&state, uuid::Uuid::nil());
    assert_eq!(state.phase, RewindPhase::MessageSelect);
    assert!(!state.preselected);
}

#[test]
fn test_handle_rewind_cancel_preselected_dismisses_from_options() {
    // When launched preselected, Esc closes the state entirely —
    // there's no message list to step back to.
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state_for_uuid(&state, test_uuid("msg-1"));
    assert_eq!(state.phase, RewindPhase::RestoreOptions);
    assert!(state.preselected);

    assert!(
        handle_rewind_cancel(&mut state),
        "cancel must dismiss when preselected"
    );
}

#[test]
fn test_handle_rewind_confirm_nevermind_preselected_dismisses() {
    // Nevermind on a preselected launch dismisses.
    let state = make_state_with_messages(2);
    let mut state = build_rewind_state_for_uuid(&state, test_uuid("msg-1"));
    state.option_selected = (state.available_options.len() as i32) - 1;
    assert_eq!(
        state.available_options[state.option_selected as usize],
        RestoreType::Nevermind
    );
    let outcome = handle_rewind_confirm(&mut state);
    assert_eq!(outcome, ConfirmOutcome::Dismiss);
}
