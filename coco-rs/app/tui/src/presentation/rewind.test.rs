use super::*;
use pretty_assertions::assert_eq;

use crate::i18n::locale_test_guard;
use crate::state::rewind::SummarizeDirection;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;

fn message(id: &str, text: &str) -> RewindableMessage {
    // Tests use stable string ids; derive the v5 uuid the cell-push
    // helper would have produced so assertions can compare against
    // `RewindableMessage.message_id: Uuid` structurally.
    let message_id = if id.is_empty() {
        uuid::Uuid::nil()
    } else {
        crate::transcript::derive::id_to_uuid(id)
    };
    RewindableMessage {
        message_id,
        message_index: 0,
        display_text: text.to_string(),
        relative_time: "2 minutes ago".to_string(),
        permission_mode: None,
        diff_stats: None,
        can_restore_code: None,
        is_current_prompt: false,
    }
}

fn state_with_messages(messages: Vec<RewindableMessage>) -> RewindState {
    RewindState {
        phase: RewindPhase::MessageSelect,
        messages,
        selected: 0,
        option_selected: 0,
        available_options: Vec::new(),
        diff_stats: None,
        diff_stats_message_id: None,
        file_history_enabled: true,
        has_file_changes: false,
        allow_summarize_up_to: false,
        summarize_feedback: String::new(),
        pending_summarize: None,
        preselected: false,
    }
}

#[test]
fn rewind_message_select_renders_empty_state_when_only_current_row_exists() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut current = message("", "(current)");
    current.is_current_prompt = true;
    let state = state_with_messages(vec![current]);

    let (title, body, border) = rewind_surface_content(&state, UiStyles::new(&theme));

    assert_eq!(title, " Rewind ");
    assert_eq!(border, theme.accent);
    assert!(body.contains("Nothing to rewind to yet."));
    assert!(body.contains("Esc Close"));
}

#[test]
fn rewind_message_select_renders_current_row_marker() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut current = message("", "(current)");
    current.is_current_prompt = true;
    let mut state = state_with_messages(vec![message("msg-1", "older"), current]);
    state.selected = 1;

    let (_, body, _) = rewind_surface_content(&state, UiStyles::new(&theme));

    assert!(body.contains("> (current)"));
    assert!(body.contains("older (2 minutes ago)"));
}

#[test]
fn rewind_message_select_renders_diff_metadata_and_scroll_hint() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut messages = (0..9)
        .map(|i| message(&format!("msg-{i}"), &format!("message {i}")))
        .collect::<Vec<_>>();
    messages[6].diff_stats = Some(DiffStatsPreview {
        insertions: 3,
        deletions: 1,
        file_paths: vec!["src/main.rs".to_string()],
    });
    messages[6].can_restore_code = Some(true);
    let mut state = state_with_messages(messages);
    state.selected = 6;

    let (_, body, _) = rewind_surface_content(&state, UiStyles::new(&theme));

    assert!(body.contains("Restore the code and/or conversation"));
    assert!(body.contains("> message 6 (2 minutes ago)"));
    assert!(body.contains("main.rs changed +3 -1"));
    assert!(body.contains("(7/9)"));
    assert!(!body.contains("message 0"));
}

#[test]
fn rewind_message_select_renders_no_changes_and_no_restore_metadata() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut messages = vec![
        message("msg-1", "no changes"),
        message("msg-2", "no restore"),
    ];
    messages[0].diff_stats = Some(DiffStatsPreview::default());
    messages[0].can_restore_code = Some(true);
    messages[1].can_restore_code = Some(false);
    let state = state_with_messages(messages);

    let (_, body, _) = rewind_surface_content(&state, UiStyles::new(&theme));

    assert!(body.contains("No code changes"));
    assert!(body.contains("⚠ No code restore"));
}

#[test]
fn rewind_restore_options_describes_code_restore_and_manual_warning() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = state_with_messages(vec![message("msg-1", "fix bug")]);
    state.phase = RewindPhase::RestoreOptions;
    state.available_options = vec![
        RestoreType::Both,
        RestoreType::ConversationOnly,
        RestoreType::Nevermind,
    ];
    state.diff_stats = Some(DiffStatsPreview {
        insertions: 10,
        deletions: 4,
        file_paths: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
    });
    state.has_file_changes = true;

    let (_, body, _) = rewind_surface_content(&state, UiStyles::new(&theme));

    assert!(body.contains("Confirm you want to restore"));
    assert!(body.contains("> Restore code and conversation"));
    assert!(body.contains("The conversation will be forked."));
    assert!(body.contains("The code will be restored: +10 -4 in main.rs and lib.rs."));
    assert!(body.contains("Rewinding does not affect files edited manually or via bash."));
}

#[test]
fn rewind_code_restore_file_labels_match_ts_counts() {
    assert_eq!(file_label(&DiffStatsPreview::default()), None);
    assert_eq!(
        file_label(&DiffStatsPreview {
            file_paths: vec!["src/main.rs".to_string()],
            ..Default::default()
        }),
        Some("main.rs".to_string())
    );
    assert_eq!(
        file_label(&DiffStatsPreview {
            file_paths: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            ..Default::default()
        }),
        Some("main.rs and lib.rs".to_string())
    );
    assert_eq!(
        file_label(&DiffStatsPreview {
            file_paths: vec![
                "src/main.rs".to_string(),
                "src/lib.rs".to_string(),
                "src/bin.rs".to_string(),
            ],
            ..Default::default()
        }),
        Some("main.rs and 2 other files".to_string())
    );
}

#[test]
fn rewind_restore_options_clamps_negative_selection() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = state_with_messages(vec![message("msg-1", "fix bug")]);
    state.phase = RewindPhase::RestoreOptions;
    state.selected = -5;
    state.option_selected = -2;
    state.available_options = vec![RestoreType::Both, RestoreType::Nevermind];

    let (_, body, _) = rewind_surface_content(&state, UiStyles::new(&theme));

    assert!(body.contains("│ fix bug"));
    assert!(body.contains("> Restore code and conversation"));
}

#[test]
fn rewind_summarize_feedback_and_confirming_use_pending_state() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let mut state = state_with_messages(vec![message("msg-1", "fix bug")]);
    state.phase = RewindPhase::SummarizeFeedback;

    let (_, empty_body, _) = rewind_surface_content(&state, UiStyles::new(&theme));
    assert!(empty_body.contains("add context (optional)"));
    assert!(empty_body.contains("(empty submit cancels)"));

    state.summarize_feedback = "keep errors".to_string();
    let (_, typed_body, _) = rewind_surface_content(&state, UiStyles::new(&theme));
    assert!(typed_body.contains("> keep errors"));

    state.phase = RewindPhase::Confirming;
    state.pending_summarize = Some(SummarizeDirection::From);
    let (_, confirming_body, _) = rewind_surface_content(&state, UiStyles::new(&theme));
    assert!(confirming_body.contains("Summarizing"));
}
