use super::*;
use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use std::sync::Arc;
use uuid::Uuid;

use crate::i18n::locale_test_guard;
use crate::presentation::streaming::StreamingTailView;
use crate::state::session::ToolExecution;
use crate::state::session::ToolStatus;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptState;
use crate::state::ui::StreamingState;
use crate::theme::Theme;
use crate::transcript::derive::test_helpers::{
    assistant_text_cell, assistant_thinking_cell_with_metadata, context_usage_cell, info_cell,
    tool_result_cell, tool_use_cell, user_text_cell,
};
use crate::widgets::TranscriptStateWidget;
use coco_tui_ui::style::UiStyles;

/// Push pre-built cells into `state.session.transcript` so the chat
/// widget and modal render them. Each fixture cell carries its own
/// engine `Message`, so `on_message_appended` rederives cells the
/// same way the runtime does.
fn push_cells(state: &mut AppState, cells: impl IntoIterator<Item = RenderedCell>) {
    for cell in cells {
        state.session.transcript.on_message_appended(cell.source);
    }
}

fn slash_user_cell(text: String) -> RenderedCell {
    let uuid = Uuid::new_v4();
    let source = Arc::new(coco_messages::Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::user_text(&text),
        uuid,
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(coco_messages::MessageOrigin::SlashCommand),
        parent_tool_use_id: None,
    }));
    RenderedCell {
        message_uuid: uuid,
        kind: CellKind::UserText { text },
        source,
    }
}

fn compact_summary_cell(text: &str) -> RenderedCell {
    let uuid = Uuid::new_v4();
    let source = Arc::new(coco_messages::Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::user_text(text),
        uuid,
        timestamp: String::new(),
        is_visible_in_transcript_only: true,
        is_virtual: false,
        is_compact_summary: true,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    }));
    RenderedCell {
        message_uuid: uuid,
        kind: CellKind::UserText {
            text: text.to_string(),
        },
        source,
    }
}

#[test]
fn test_tool_output_preview_empty_output() {
    assert_eq!(tool_output_preview("", 5), ToolOutputPreview::Empty);
}

#[test]
fn test_tool_output_preview_short_output_keeps_all_lines() {
    assert_eq!(
        tool_output_preview("one\ntwo\nthree", 5),
        ToolOutputPreview::Full(vec!["one", "two", "three"])
    );
}

#[test]
fn test_tool_output_preview_long_output_keeps_head_and_tail() {
    assert_eq!(
        tool_output_preview("one\ntwo\nthree\nfour\nfive\nsix\nseven\neight", 5),
        ToolOutputPreview::Truncated {
            head: vec!["one", "two"],
            omitted: 4,
            tail: vec!["seven", "eight"],
        }
    );
}

#[test]
fn test_tool_output_preview_one_row_budget_reports_omitted_lines() {
    assert_eq!(
        tool_output_preview("one\ntwo\nthree", 1),
        ToolOutputPreview::Truncated {
            head: vec![],
            omitted: 3,
            tail: vec![],
        }
    );
}

#[test]
fn transcript_modal_widget_renders_empty_state_and_footer_without_show_all() {
    let _locale = locale_test_guard("en");
    let app_state = AppState::default();
    let state = TranscriptState::new();
    let body = render_transcript_text(&app_state, &state, 72, 8);

    assert!(body.contains("No messages yet."));
    assert!(body.contains("ctrl+o toggle"));
    assert!(body.contains("PgUp/PgDn page"));
    assert!(body.contains("Esc/q quit"));
    assert!(!body.contains("ctrl+e"));
    assert!(!body.contains("show all"));
}

#[test]
fn snapshot_transcript_modal_empty_footer() {
    let _locale = locale_test_guard("en");
    let app_state = AppState::default();
    let state = TranscriptState::new();

    insta::assert_snapshot!(
        "transcript_modal_empty_footer",
        render_transcript_text(&app_state, &state, 72, 8)
    );
}

#[test]
fn snapshot_transcript_modal_selected_tool_preview() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            user_text_cell(Uuid::new_v4(), "inspect src/lib.rs"),
            tool_use_cell("call-1", "Read", serde_json::json!({})),
            tool_result_cell(
                "call-1",
                "Read",
                "pub fn alpha() {}\n\
                 pub fn beta() {}\n\
                 pub fn gamma() {}\n\
                 pub fn delta() {}\n\
                 pub fn epsilon() {}\n\
                 pub fn zeta() {}",
            ),
            assistant_text_cell("Found the helpers."),
        ],
    );
    let state = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));

    insta::assert_snapshot!(
        "transcript_modal_selected_tool_preview",
        render_transcript_text(&app_state, &state, 84, 14)
    );
}

#[test]
fn snapshot_transcript_modal_expanded_thinking_cell() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    // Push messages directly (`push_cells` rederives from
    // `cell.source`; thinking metadata lives on the cell, not the
    // source message, so we drive the engine API the same way the
    // runtime does — append messages, then stamp reasoning tokens
    // via `record_reasoning_tokens`).
    let (thinking_cell, thinking_meta) = assistant_thinking_cell_with_metadata(
        "The user wants me to run `ls` in the current working directory.\n\
         I should call the Bash tool and then summarize the result.",
        1300,
        15,
    );
    let thinking_uuid = thinking_cell.message_uuid;
    push_cells(
        &mut app_state,
        [
            user_text_cell(Uuid::new_v4(), "bash ls"),
            thinking_cell,
            assistant_text_cell("I'll list the current directory."),
        ],
    );
    // Stamp the reasoning metadata in the side-cache by the thinking
    // cell's message uuid so the renderer surfaces the
    // `Thinking · 1.3s · 15` header (mirrors the production path
    // where `on_turn_completed` inserts into `reasoning_metadata`).
    app_state
        .session
        .reasoning_metadata
        .insert(thinking_uuid, thinking_meta);
    let state = TranscriptState::new_with_anchor(Some(TranscriptCellId::message(
        1,
        thinking_uuid.to_string(),
    )));

    insta::assert_snapshot!(
        "transcript_modal_expanded_thinking_cell",
        render_transcript_text(&app_state, &state, 96, 12)
    );
}

#[test]
fn transcript_modal_collapsed_tool_keeps_header_and_head_tail_preview() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [tool_result_cell(
            "call-1",
            "Glob",
            "common/error/README.md\n\
             common/otel/README.md\n\
             retrieval/README.md\n\
             services/lsp/README.md\n\
             services/mcp-types/README.md\n\
             utils/file-search/README.md\n\
             utils/git/README.md\n\
             utils/shell-parser/README.md\n\
             utils/stdio-to-uds/README.md\n\
             utils/stream-parser/README.md\n\
             exec/apply-patch/tests/fixtures/scenarios/README.md\n\
             vercel-ai/README.md\n\
             core/system-reminder/README.md",
        )],
    );
    let mut state = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));
    state
        .collapsed_cell_ids
        .insert(TranscriptCellId::tool("call-1"));

    let body = render_transcript_text(&app_state, &state, 96, 14);

    assert!(body.contains("● Glob"));
    assert!(body.contains("└ common/error/README.md"));
    assert!(body.contains("common/otel/README.md"));
    assert!(body.contains("… +9 lines (ctrl+o to expand)"));
    assert!(body.contains("vercel-ai/README.md"));
    assert!(body.contains("core/system-reminder/README.md"));
    assert!(!body.contains("retrieval/README.md"));
}

#[test]
fn transcript_text_messages_are_full_and_not_expandable() {
    let _locale = locale_test_guard("en");
    let repeated = "one\ntwo\nthree\nfour\nfive\nsix";
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [assistant_text_cell(repeated), assistant_text_cell(repeated)],
    );
    let state = TranscriptState::new();

    let body = render_transcript_text(&app_state, &state, 84, 18);

    assert!(body.contains("six"));
    assert!(!body.contains(TRANSCRIPT_TRUNCATED_HINT));
    assert!(transcript_expandable_cell_ids(&app_state).is_empty());
}

#[test]
fn snapshot_transcript_modal_expanded_truncation_tail() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            tool_use_cell("call-1", "Read", serde_json::json!({})),
            tool_result_cell(
                "call-1",
                "Read",
                &(0..=TRANSCRIPT_EXPANDED_CELL_LINE_CAP)
                    .map(|i| format!("expanded-line-{i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ],
    );
    let mut state = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));
    state.scroll = crate::state::transcript::TranscriptScrollPosition::Absolute(
        TRANSCRIPT_EXPANDED_CELL_LINE_CAP.saturating_sub(4),
    );

    insta::assert_snapshot!(
        "transcript_modal_expanded_truncation_tail",
        render_transcript_text(&app_state, &state, 84, 12)
    );
}

#[test]
fn snapshot_transcript_modal_streaming_tail() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [user_text_cell(Uuid::new_v4(), "summarize status")],
    );
    let mut streaming = StreamingState::new();
    streaming.append_text("Working through the transcript pager changes.");
    streaming.reveal_all();
    app_state.ui.streaming = Some(streaming);
    let state = TranscriptState::new();

    insta::assert_snapshot!(
        "transcript_modal_streaming_tail",
        render_transcript_text(&app_state, &state, 84, 12)
    );
}

#[test]
fn transcript_modal_renders_slash_command_without_raw_tags() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            slash_user_cell(coco_messages::format_command_input("compact", "")),
            slash_user_cell(coco_messages::format_local_command_stdout(
                "Compacted (12 -> 7 tokens, saved 5 / 41.7%; Ctrl+O to see full summary)",
            )),
        ],
    );
    let state = TranscriptState::new();
    let body = render_transcript_text(&app_state, &state, 96, 10);

    assert!(body.contains("❯ /compact"));
    assert!(body.contains("Compacted (12 -> 7 tokens"));
    assert!(!body.contains("<command-name>"));
    assert!(!body.contains("<local-command-stdout>"));
}

#[test]
fn transcript_modal_keeps_compact_summary_available_for_ctrl_o() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [compact_summary_cell(
            "Summary:\nEarlier compacted context remains available here.",
        )],
    );
    let state = TranscriptState::new();
    let body = render_transcript_text(&app_state, &state, 96, 10);

    assert!(body.contains("Summary:"));
    assert!(body.contains("Earlier compacted context remains available here."));
}

fn render_transcript_text(
    state: &AppState,
    transcript: &TranscriptState,
    width: u16,
    height: u16,
) -> String {
    let theme = Theme::default();
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let mut layout = crate::widgets::TranscriptLayoutIndex::default();
    TranscriptStateWidget::new(state, transcript, &mut layout, UiStyles::new(&theme))
        .render(area, &mut buffer);
    buffer
        .content
        .chunks(width as usize)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect::<Vec<String>>()
        .join("\n")
}

fn projection_cells(cells: &[RenderedCell], show_system_reminders: bool) -> Vec<TranscriptCell> {
    transcript_projection(
        cells,
        TranscriptProjectionOptions {
            show_system_reminders,
            show_compact_internals: false,
        },
    )
    .cells
}

#[test]
fn transcript_projection_hides_compact_internals_unless_requested() {
    let boundary = crate::transcript::derive::message_to_cells(Arc::new(
        coco_messages::create_compact_boundary_message(10, 4),
    ));
    let summary = vec![compact_summary_cell("Summary:\nHidden in default chat")];
    let cells = boundary.into_iter().chain(summary).collect::<Vec<_>>();

    let hidden = transcript_projection(
        &cells,
        TranscriptProjectionOptions {
            show_system_reminders: false,
            show_compact_internals: false,
        },
    );
    assert!(hidden.cells.is_empty());

    let visible = transcript_projection(
        &cells,
        TranscriptProjectionOptions {
            show_system_reminders: false,
            show_compact_internals: true,
        },
    );
    assert_eq!(visible.cells.len(), 2);
}

#[test]
fn transcript_projection_distinguishes_meta_and_regular_messages() {
    let cells = vec![
        info_cell("system", "system reminder"),
        user_text_cell(Uuid::new_v4(), "hello"),
    ];

    assert_eq!(
        projection_cells(&cells, false),
        vec![
            TranscriptCell::MetaPreview { index: 0 },
            TranscriptCell::Cell { index: 1 },
        ]
    );
    assert_eq!(
        projection_cells(&cells, true),
        vec![
            TranscriptCell::Cell { index: 0 },
            TranscriptCell::Cell { index: 1 },
        ]
    );
}

#[test]
fn transcript_projection_renders_context_usage_as_full_cell_not_meta() {
    // `/context` is first-class inline content: it must never collapse to a
    // `# [context]` meta preview, regardless of the system-reminder toggle.
    let cells = vec![context_usage_cell()];
    assert_eq!(
        projection_cells(&cells, /*show_system_reminders*/ false),
        vec![TranscriptCell::Cell { index: 0 }]
    );
    assert_eq!(
        projection_cells(&cells, /*show_system_reminders*/ true),
        vec![TranscriptCell::Cell { index: 0 }]
    );
}

#[test]
fn transcript_projection_renders_user_interruption_as_content() {
    let cells = crate::transcript::derive::message_to_cells(Arc::new(
        coco_messages::create_user_interruption_system_message(false),
    ));

    assert_eq!(
        projection_cells(&cells, /*show_system_reminders*/ false),
        vec![TranscriptCell::Cell { index: 0 }]
    );
    assert_eq!(
        projection_cells(&cells, /*show_system_reminders*/ true),
        vec![TranscriptCell::Cell { index: 0 }]
    );
}

#[test]
fn transcript_projection_groups_parallel_tool_runs() {
    let cells = vec![
        tool_use_cell("tool-1", "Read", serde_json::json!({})),
        info_cell("hint", "tool reminder"),
        tool_use_cell("tool-2", "Read", serde_json::json!({})),
        assistant_text_cell("done"),
    ];

    assert_eq!(
        projection_cells(&cells, false),
        vec![
            TranscriptCell::ToolBatch {
                start: 0,
                end: 3,
                count: 2,
            },
            TranscriptCell::ToolCall {
                invocation: Some(0),
                result: None,
                call_id: Some("tool-1".to_string()),
            },
            TranscriptCell::MetaPreview { index: 1 },
            TranscriptCell::ToolCall {
                invocation: Some(2),
                result: None,
                call_id: Some("tool-2".to_string()),
            },
            TranscriptCell::Cell { index: 3 },
        ]
    );
}

#[test]
fn transcript_projection_groups_parallel_tool_runs_with_results() {
    let cells = vec![
        tool_use_cell("tool-1", "Glob", serde_json::json!({"pattern": "**/*.rs"})),
        tool_use_cell("tool-2", "Glob", serde_json::json!({"pattern": "**/*.md"})),
        tool_result_cell("tool-1", "Glob", "src/lib.rs"),
        tool_result_cell("tool-2", "Glob", "README.md"),
    ];

    assert_eq!(
        projection_cells(&cells, false),
        vec![
            TranscriptCell::ToolBatch {
                start: 0,
                end: 2,
                count: 2,
            },
            TranscriptCell::ToolCall {
                invocation: Some(0),
                result: Some(2),
                call_id: Some("tool-1".to_string()),
            },
            TranscriptCell::ToolCall {
                invocation: Some(1),
                result: Some(3),
                call_id: Some("tool-2".to_string()),
            },
        ]
    );
}

#[test]
fn snapshot_transcript_modal_parallel_glob_results() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            tool_use_cell("tool-1", "Glob", serde_json::json!({"pattern": "**/*.rs"})),
            tool_use_cell("tool-2", "Glob", serde_json::json!({"pattern": "**/*.md"})),
            tool_result_cell("tool-1", "Glob", "src/lib.rs\nsrc/main.rs"),
            tool_result_cell("tool-2", "Glob", "README.md\nAGENTS.md"),
        ],
    );
    let state = TranscriptState::new();

    insta::assert_snapshot!(
        "transcript_modal_parallel_glob_results",
        render_transcript_text(&app_state, &state, 84, 14)
    );
}

#[test]
fn transcript_projection_pairs_tool_use_with_result_by_call_id() {
    let cells = vec![
        user_text_cell(Uuid::new_v4(), "list files"),
        tool_use_cell("call-1", "Read", serde_json::json!({})),
        assistant_text_cell("checking"),
        tool_result_cell("call-1", "Read", "alpha\nbeta"),
    ];

    assert_eq!(
        projection_cells(&cells, false),
        vec![
            TranscriptCell::Cell { index: 0 },
            TranscriptCell::ToolCall {
                invocation: Some(1),
                result: Some(3),
                call_id: Some("call-1".to_string()),
            },
            TranscriptCell::Cell { index: 2 },
        ]
    );
}

#[test]
fn transcript_modal_widget_highlights_anchor_cell_and_keeps_it_expanded() {
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            user_text_cell(Uuid::new_v4(), "list"),
            tool_use_cell("call-1", "Read", serde_json::json!({})),
            tool_result_cell("call-1", "Read", "alpha\nbeta"),
        ],
    );
    let state = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));
    let body = render_transcript_text(&app_state, &state, 80, 12);

    assert!(body.contains("▶"));
    assert!(body.contains("Read"));
    assert!(body.contains("alpha"));
}

#[test]
fn transcript_modal_expands_tool_cells_by_default() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            tool_use_cell("old-call", "Read", serde_json::json!({})),
            tool_result_cell(
                "old-call",
                "Read",
                "old-alpha\nold-beta\nold-gamma\nold-delta\nold-epsilon\nold-zeta",
            ),
            tool_use_cell("new-call", "Read", serde_json::json!({})),
            tool_result_cell(
                "new-call",
                "Read",
                "new-alpha\nnew-beta\nnew-gamma\nnew-delta\nnew-epsilon\nnew-zeta",
            ),
        ],
    );
    let state = TranscriptState::new();
    let body = render_transcript_text(&app_state, &state, 80, 24);

    assert!(body.contains("old-alpha"));
    assert!(body.contains("new-alpha"));
    assert!(body.contains("old-zeta"));
    assert!(body.contains("new-zeta"));
    assert!(!body.contains(TRANSCRIPT_TRUNCATED_HINT));
}

#[test]
fn transcript_modal_caps_expanded_tool_result_lines() {
    let _locale = locale_test_guard("en");
    let mut app_state = AppState::default();
    push_cells(
        &mut app_state,
        [
            tool_use_cell("call-1", "Read", serde_json::json!({})),
            tool_result_cell(
                "call-1",
                "Read",
                &(0..=TRANSCRIPT_EXPANDED_CELL_LINE_CAP)
                    .map(|i| format!("line-{i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ],
    );
    let state = TranscriptState::new_with_anchor(Some(TranscriptCellId::tool("call-1")));

    let body = render_transcript_text(
        &app_state,
        &state,
        80,
        (TRANSCRIPT_EXPANDED_CELL_LINE_CAP + 8) as u16,
    );

    assert!(body.contains("line-0"));
    assert!(!body.contains(&format!("line-{TRANSCRIPT_EXPANDED_CELL_LINE_CAP}")));
    // Reader caps at the per-cell line budget; the overflow collapses into a
    // SINGLE trailing marker. One row of the budget is reserved for that marker,
    // so the last two lines (cap-1 and cap) are folded into "+2 lines".
    assert!(body.contains("+2 lines"));
}

#[test]
fn active_transcript_cell_prioritizes_streaming_then_in_flight_tools() {
    let mut streaming = StreamingState::new();
    streaming.append_text("hello");
    streaming.reveal_all();
    let tools = vec![tool_execution(ToolStatus::Running)];

    // Streaming text always wins.
    assert_eq!(
        active_transcript_cell(Some(&streaming), true, &tools),
        Some(ActiveTranscriptCell::Streaming(StreamingTailView {
            assistant_text: Some("hello"),
            thinking_tokens: None,
        }))
    );
    // No streaming + an in-flight tool → inline `● Tool(args)` rows.
    assert_eq!(
        active_transcript_cell(None, true, &tools),
        Some(ActiveTranscriptCell::InFlightTools)
    );
    // Completed tool → no active cell (its committed header now pairs + paints).
    assert_eq!(
        active_transcript_cell(None, true, &[tool_execution(ToolStatus::Completed)]),
        None
    );
}

#[test]
fn active_transcript_cell_in_flight_regardless_of_message_uuid() {
    // The committed `● Tool` header is held out of scrollback until the result
    // pairs, so a running tool yields the inline row whether or not its owning
    // assistant message has committed (`message_uuid` set). Guards against
    // re-introducing a `message_uuid` gate that would blank the multi-minute
    // `Agent`-run window.
    for tool in [
        tool_execution(ToolStatus::Running), // message_uuid: None
        committed_tool_execution(ToolStatus::Running), // message_uuid: Some
    ] {
        assert_eq!(
            active_transcript_cell(None, true, &[tool]),
            Some(ActiveTranscriptCell::InFlightTools)
        );
    }
}

#[test]
fn transcript_presentation_appends_active_streaming_after_committed_cells() {
    let streaming = StreamingState::new();
    let cells = vec![user_text_cell(Uuid::new_v4(), "hello")];
    let presentation = transcript_presentation(TranscriptPresentationInput {
        cells: &cells,
        options: TranscriptProjectionOptions {
            show_system_reminders: false,
            show_compact_internals: false,
        },
        streaming: Some(&streaming),
        show_thinking: true,
        tool_executions: &[],
    });

    assert_eq!(
        presentation.cells,
        vec![
            TranscriptSourceCell::Committed(TranscriptCell::Cell { index: 0 }),
            TranscriptSourceCell::Active(ActiveTranscriptCell::Streaming(StreamingTailView {
                assistant_text: None,
                thinking_tokens: None,
            })),
        ]
    );
}

#[test]
fn transcript_presentation_appends_in_flight_tools_when_uncommitted() {
    let cells = vec![assistant_text_cell("done")];
    let tools = vec![tool_execution(ToolStatus::Queued)];
    let presentation = transcript_presentation(TranscriptPresentationInput {
        cells: &cells,
        options: TranscriptProjectionOptions {
            show_system_reminders: false,
            show_compact_internals: false,
        },
        streaming: None,
        show_thinking: true,
        tool_executions: &tools,
    });

    assert_eq!(
        presentation.cells,
        vec![
            TranscriptSourceCell::Committed(TranscriptCell::Cell { index: 0 }),
            TranscriptSourceCell::Active(ActiveTranscriptCell::InFlightTools),
        ]
    );
}

#[test]
fn transcript_presentation_appends_in_flight_tools_for_committed_running_tool() {
    let cells = vec![assistant_text_cell("done")];
    let tools = vec![committed_tool_execution(ToolStatus::Running)];
    let presentation = transcript_presentation(TranscriptPresentationInput {
        cells: &cells,
        options: TranscriptProjectionOptions {
            show_system_reminders: false,
            show_compact_internals: false,
        },
        streaming: None,
        show_thinking: true,
        tool_executions: &tools,
    });

    assert_eq!(
        presentation.cells,
        vec![
            TranscriptSourceCell::Committed(TranscriptCell::Cell { index: 0 }),
            TranscriptSourceCell::Active(ActiveTranscriptCell::InFlightTools),
        ]
    );
}

#[test]
fn transcript_presentation_omits_active_cell_when_idle() {
    let cells = vec![assistant_text_cell("done")];
    let tools = vec![tool_execution(ToolStatus::Completed)];
    let presentation = transcript_presentation(TranscriptPresentationInput {
        cells: &cells,
        options: TranscriptProjectionOptions {
            show_system_reminders: false,
            show_compact_internals: false,
        },
        streaming: None,
        show_thinking: true,
        tool_executions: &tools,
    });

    assert_eq!(
        presentation.cells,
        vec![TranscriptSourceCell::Committed(TranscriptCell::Cell {
            index: 0
        })]
    );
}

fn tool_execution(status: ToolStatus) -> ToolExecution {
    ToolExecution {
        call_id: "call".to_string(),
        name: "Read".to_string(),
        status,
        started_at: std::time::Instant::now(),
        completed_at: None,
        description: None,
        input_preview: None,
        streaming_input: None,
        // Unstamped (`None`) ⇒ mid-stream / uncommitted: the owning assistant
        // message hasn't landed, so this renders as an inline InFlightTools row.
        message_uuid: None,
    }
}

/// Same fixture but stamped with a `message_uuid`, i.e. the owning assistant
/// message has committed and a `● Tool` header already renders for it.
fn committed_tool_execution(status: ToolStatus) -> ToolExecution {
    ToolExecution {
        message_uuid: Some(Uuid::new_v4()),
        ..tool_execution(status)
    }
}
