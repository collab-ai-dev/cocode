use super::toggle;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptScrollPosition;
use crate::transcript::derive::test_helpers;

#[test]
fn toggle_opens_transcript_when_no_surface_active() {
    let mut state = AppState::new();
    toggle(&mut state);
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::Transcript(_))
    ));
}

#[test]
fn toggle_closes_transcript_when_already_open() {
    let mut state = AppState::new();
    toggle(&mut state);
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::Transcript(_))
    ));
    toggle(&mut state);
    assert!(!state.ui.has_active_surface());
}

#[test]
fn transcript_modal_defaults_to_cell_pager_state() {
    let mut state = AppState::new();
    toggle(&mut state);
    let state = state.ui.modal.as_ref().expect("transcript opened");
    let ModalState::Transcript(t) = state else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Top);
    assert_eq!(t.selected_cell_id, None);
    assert!(t.collapsed_cell_ids.is_empty());
}

#[test]
fn toggle_opens_transcript_on_latest_expandable_cell() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "old", "Read", "old\nlines", false);
    test_helpers::push_tool_result(&mut state.session, "new", "Read", "new\nlines", false);

    toggle(&mut state);

    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("new"))
    );
    assert_eq!(
        t.scroll,
        TranscriptScrollPosition::anchor(TranscriptCellId::tool("new"))
    );
}

#[test]
fn select_and_enter_toggle_collapsed_cell() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "call-1", "Read", "alpha\nbeta", false);
    toggle(&mut state);

    assert!(super::select_expandable(&mut state, 1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("call-1"))
    );

    assert!(super::toggle_selected_cell(&mut state));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert!(
        t.collapsed_cell_ids
            .contains(&TranscriptCellId::tool("call-1"))
    );

    assert!(super::toggle_selected_cell(&mut state));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert!(
        !t.collapsed_cell_ids
            .contains(&TranscriptCellId::tool("call-1"))
    );
}

#[test]
fn select_expandable_wraps_at_edges() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "first", "Read", "one\ntwo", false);
    test_helpers::push_tool_result(&mut state.session, "last", "Read", "three\nfour", false);
    toggle(&mut state);

    assert!(super::select_expandable(&mut state, 1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("first"))
    );

    assert!(super::select_expandable(&mut state, -1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("last"))
    );
}

#[test]
fn select_expandable_anchors_selected_cell_from_current_scroll() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "call-1", "Read", "alpha\nbeta", false);
    toggle(&mut state);
    assert!(super::scroll_lines(&mut state, 40));

    assert!(super::select_expandable(&mut state, 1));

    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.scroll,
        TranscriptScrollPosition::anchor(TranscriptCellId::tool("call-1"))
    );
}

#[test]
fn transcript_scroll_uses_modal_state() {
    let mut state = AppState::new();
    toggle(&mut state);

    assert!(super::scroll_lines(&mut state, 5));
    assert_eq!(state.ui.scroll_offset, 0);
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Absolute(5));
}

#[test]
fn transcript_page_uses_terminal_size() {
    let mut state = AppState::new();
    state.ui.terminal_size = ratatui::layout::Size::new(100, 21);
    toggle(&mut state);
    let Some(ModalState::Transcript(_)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };

    assert!(super::page(&mut state, 1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Absolute(17));

    assert!(super::page(&mut state, -1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Top);
}

#[test]
fn search_matches_rendered_markdown_plain_text_and_navigates() {
    let mut state = AppState::new();
    test_helpers::push_assistant_text(
        &mut state.session,
        "This is **really** important.\nAnother REALLY useful line.",
    );
    toggle(&mut state);

    assert!(super::search_start(&mut state));
    for c in "really".chars() {
        assert!(super::search_insert(&mut state, c));
    }

    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(transcript.search.matches.len(), 2);
    assert_eq!(transcript.search.status(), (1, 2));
    assert!(matches!(
        transcript.scroll,
        TranscriptScrollPosition::Anchor { .. }
    ));

    assert!(super::search_submit(&mut state));
    assert!(super::search_navigate(&mut state, 1));
    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(transcript.search.status(), (2, 2));
}

#[test]
fn search_index_rebuilds_after_transcript_revision_changes() {
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "needle one");
    test_helpers::push_assistant_text(&mut state.session, "stable context");
    toggle(&mut state);
    super::search_start(&mut state);
    for c in "needle".chars() {
        super::search_insert(&mut state, c);
    }

    test_helpers::push_assistant_text(&mut state.session, "needle two");
    assert!(super::search_navigate(&mut state, 1));

    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(transcript.search.matches.len(), 2);
    assert_eq!(
        transcript.search.reused_entries_last_build, 2,
        "an append must move stable per-cell corpus entries without rerendering them"
    );
}

#[test]
fn search_reveal_expands_a_collapsed_cell_and_uses_wrapped_row_offset() {
    let mut state = AppState::new();
    state.ui.terminal_size = ratatui::layout::Size::new(12, 20);
    test_helpers::push_assistant_text(
        &mut state.session,
        "alpha bravo cobra delta epsilon wrapped-needle",
    );
    toggle(&mut state);
    super::search_start(&mut state);
    let cell_id = match state.ui.modal.as_mut() {
        Some(ModalState::Transcript(transcript)) => {
            let id = transcript
                .search
                .entries
                .first()
                .expect("search entry")
                .cell_id
                .clone();
            transcript.collapsed_cell_ids.insert(id.clone());
            id
        }
        _ => panic!("expected transcript"),
    };

    for c in "wrapped-needle".chars() {
        super::search_insert(&mut state, c);
    }

    let Some(ModalState::Transcript(transcript)) = state.ui.modal.as_ref() else {
        panic!("expected transcript");
    };
    assert!(!transcript.collapsed_cell_ids.contains(&cell_id));
    let current = transcript.search.current_match().expect("match");
    assert!(
        current.row_offset > 0,
        "long-line match must target a wrapped row"
    );
}

#[test]
fn search_index_rebuilds_for_width_and_stream_generation_independently() {
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "needle");
    state.ui.streaming = Some(crate::state::ui::StreamingState::new());
    toggle(&mut state);
    super::search_start(&mut state);
    for c in "needle".chars() {
        super::search_insert(&mut state, c);
    }
    let first = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.indexed_revision.unwrap(),
        _ => panic!("expected transcript"),
    };

    state.ui.terminal_size.width = state.ui.terminal_size.width.saturating_sub(4);
    super::search_navigate(&mut state, 1);
    let after_width = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.indexed_revision.unwrap(),
        _ => panic!("expected transcript"),
    };
    assert_ne!(first.width, after_width.width);
    assert_eq!(first.stream, after_width.stream);
    let reused_after_width = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.reused_entries_last_build,
        _ => panic!("expected transcript"),
    };
    assert_eq!(reused_after_width, 0, "width changes require rewrapping");

    state
        .ui
        .streaming
        .as_mut()
        .expect("stream")
        .visible_generation += 1;
    super::search_navigate(&mut state, 1);

    let after_stream = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.indexed_revision.unwrap(),
        _ => panic!("expected transcript"),
    };
    assert_eq!(after_width.width, after_stream.width);
    assert_ne!(after_width.stream, after_stream.stream);
    let reused_after_stream = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.reused_entries_last_build,
        _ => panic!("expected transcript"),
    };
    assert_eq!(
        reused_after_stream, 1,
        "a streaming-tail update must reuse the committed cell entry"
    );
}

#[test]
fn search_index_invalidates_each_mutable_side_cache_precisely() {
    let mut state = AppState::new();
    let thinking_id =
        test_helpers::push_assistant_thinking(&mut state.session, "deliberation", 1_000, 10);
    test_helpers::push_tool_use(&mut state.session, "status-call", "Read", "src/lib.rs");
    let completed_at = std::time::Instant::now();
    state
        .session
        .tool_executions
        .push(crate::state::ToolExecution {
            call_id: "status-call".to_string(),
            name: "Read".to_string(),
            status: crate::state::ToolStatus::Completed,
            started_at: completed_at,
            completed_at: Some(completed_at),
            description: None,
            input_preview: Some("src/lib.rs".to_string()),
            streaming_input: None,
            message_uuid: None,
        });
    test_helpers::push_tool_result(&mut state.session, "agent-call", "Agent", "done", false);
    state.session.insert_subagent_summary(
        "agent-call".to_string(),
        crate::state::session::SubagentRunSummary {
            agent_type: "Explore".to_string(),
            tool_count: 2,
            duration_ms: 1_000,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
            cost_usd: 0.01,
            succeeded: true,
        },
    );

    toggle(&mut state);
    assert!(super::search_start(&mut state));
    let entry_count = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.entries.len(),
        _ => panic!("expected transcript"),
    };
    assert_eq!(
        entry_count, 3,
        "fixture should create three independent entries"
    );

    state.session.insert_reasoning_metadata(
        thinking_id,
        crate::state::session::ReasoningMetadata {
            duration_ms: Some(2_000),
            reasoning_tokens: 20,
        },
    );
    assert!(super::ensure_search_index(&mut state));
    let reused = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.reused_entries_last_build,
        _ => panic!("expected transcript"),
    };
    assert_eq!(reused, entry_count - 1, "reasoning metadata invalidation");

    state.session.tool_executions[0].status = crate::state::ToolStatus::Failed;
    assert!(super::ensure_search_index(&mut state));
    let reused = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.reused_entries_last_build,
        _ => panic!("expected transcript"),
    };
    assert_eq!(reused, entry_count - 1, "tool status invalidation");

    state
        .session
        .subagent_summaries
        .get_mut("agent-call")
        .expect("summary")
        .agent_type = "Plan".to_string();
    assert!(super::ensure_search_index(&mut state));
    let reused = match state.ui.modal.as_ref() {
        Some(ModalState::Transcript(transcript)) => transcript.search.reused_entries_last_build,
        _ => panic!("expected transcript"),
    };
    assert_eq!(reused, entry_count - 1, "subagent summary invalidation");
}
