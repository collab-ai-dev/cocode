use super::TRANSCRIPT_LINE_CHAR_CAP;
use super::attachment_summary_text;
use super::compact_file_reference_chip_path;
use super::in_flight_tool_lines;
use super::mention_summary_lines;
use super::nested_memory_chip_path;
use super::single_line_capped;
use super::transcript_safe_line;
use coco_messages::AttachmentMessage;
use coco_messages::CompactFileReferencePayload;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::AttachmentKind;

#[test]
fn test_transcript_safe_line_caps_single_line_output() {
    let input = "x".repeat(TRANSCRIPT_LINE_CHAR_CAP + 20);

    let rendered = transcript_safe_line(&input);

    assert_eq!(rendered.chars().count(), TRANSCRIPT_LINE_CHAR_CAP);
    assert!(rendered.ends_with('…'));
}

#[test]
fn test_single_line_capped_collapses_whitespace_without_collecting_full_input() {
    let input = format!("alpha\n{}\nomega", "beta ".repeat(TRANSCRIPT_LINE_CHAR_CAP));

    let rendered = single_line_capped(&input, 32);

    assert!(rendered.starts_with("alpha beta"));
    assert_eq!(rendered.chars().count(), 32);
    assert!(rendered.ends_with('…'));
}

#[test]
fn nested_memory_chip_path_extracts_path_for_memory_kinds_only() {
    // A nested-CLAUDE.md reminder collapses to just its `{path}` (the `Contents
    // of …:` framing + `<system-reminder>` wrapper are stripped) for the chip.
    let body = "<system-reminder>\nContents of /repo/utils/foo/CLAUDE.md:\n\n# foo rules\n</system-reminder>";
    let memory = Message::Attachment(AttachmentMessage::api(
        AttachmentKind::NestedMemory,
        LlmMessage::user_text(body),
    ));
    // No cwd → absolute path unchanged.
    assert_eq!(
        nested_memory_chip_path(&memory, None).as_deref(),
        Some("/repo/utils/foo/CLAUDE.md")
    );
    // Under cwd → relativized for the compact chip.
    assert_eq!(
        nested_memory_chip_path(&memory, Some("/repo")).as_deref(),
        Some("utils/foo/CLAUDE.md")
    );
    // Outside cwd → left absolute (e.g. a global memory file).
    assert_eq!(
        nested_memory_chip_path(&memory, Some("/other")).as_deref(),
        Some("/repo/utils/foo/CLAUDE.md")
    );

    // A non-memory attachment is left to the generic `◇` preview path.
    let other = Message::Attachment(AttachmentMessage::api(
        AttachmentKind::DateChange,
        LlmMessage::user_text("The date has changed to 2026-06-02."),
    ));
    assert_eq!(nested_memory_chip_path(&other, Some("/repo")), None);
}

#[test]
fn mention_summary_lines_render_compact_rows() {
    let _locale = crate::i18n::locale_test_guard("en");
    let theme = coco_tui_ui::theme::Theme::default();
    let styles = coco_tui_ui::style::UiStyles::new(&theme);

    let msg = Message::Attachment(AttachmentMessage::mention_summary(
        coco_messages::MentionSummaryPayload {
            items: vec![
                coco_messages::MentionSummaryItem {
                    display_path: "foo.rs".to_string(),
                    kind: coco_messages::MentionItemKind::File,
                    count: Some(3),
                    truncated: false,
                },
                coco_messages::MentionSummaryItem {
                    display_path: "dir".to_string(),
                    kind: coco_messages::MentionItemKind::Directory,
                    count: None,
                    truncated: false,
                },
            ],
        },
    ));

    let lines = mention_summary_lines(&msg, styles).expect("summary rows");
    assert_eq!(lines.len(), 2);
    let text: Vec<String> = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();
    assert!(text[0].contains("Read foo.rs (3 lines)"), "{text:?}");
    assert!(text[1].contains("Listed directory dir/"), "{text:?}");
}

#[test]
fn in_flight_tool_lines_render_header_and_skip_committed() {
    use crate::state::session::ToolExecution;
    use crate::state::session::ToolStatus;

    let theme = coco_tui_ui::theme::Theme::default();
    let styles = coco_tui_ui::style::UiStyles::new(&theme);

    let mk = |call_id: &str, preview: Option<&str>, status| ToolExecution {
        call_id: call_id.to_string(),
        name: "Read".to_string(),
        status,
        started_at: std::time::Instant::now(),
        completed_at: None,
        description: None,
        input_preview: preview.map(str::to_string),
        streaming_input: None,
        // message_uuid is irrelevant to the filter — the committed header is
        // held back until the result pairs regardless, so set it to prove that.
        message_uuid: Some(uuid::Uuid::new_v4()),
    };

    let tools = vec![
        // Still executing → its header is withheld, so render the inline row.
        mk("a", Some("CLAUDE.md"), ToolStatus::Running),
        // Finished → its committed `● Read` header now pairs + paints, so the
        // inline row must NOT duplicate it.
        mk("b", Some("other.rs"), ToolStatus::Completed),
    ];

    let lines = in_flight_tool_lines(&tools, styles);
    assert_eq!(lines.len(), 1, "only the still-running tool renders a row");
    let row: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(row.contains("● Read"), "{row:?}");
    assert!(row.contains("CLAUDE.md"), "{row:?}");
    assert!(
        !row.contains("other.rs"),
        "completed tool excluded: {row:?}"
    );
}

#[test]
fn bare_file_attachment_listing_renders_nothing() {
    // The `@-mentioned files` generator listing (`K::File` + API body) is
    // model-only metadata — it must not leak as a transcript row, and it has
    // no `MentionSummary` extras to render either.
    let msg = Message::Attachment(AttachmentMessage::api(
        AttachmentKind::File,
        LlmMessage::user_text(
            "The user @-mentioned the following file(s). Their content has been loaded into context:\n- foo.rs",
        ),
    ));
    assert_eq!(attachment_summary_text(&msg), None);

    let theme = coco_tui_ui::theme::Theme::default();
    let styles = coco_tui_ui::style::UiStyles::new(&theme);
    assert!(mention_summary_lines(&msg, styles).is_none());
}

#[test]
fn compact_file_reference_chip_path_uses_typed_payload() {
    let message = Message::Attachment(AttachmentMessage::compact_file_reference(
        CompactFileReferencePayload {
            filename: "/repo/src/lib.rs".to_string(),
            display_path: "src/lib.rs".to_string(),
        },
        LlmMessage::user_text(
            "<system-reminder>\nCalled the Read tool with the following input: {\"file_path\":\"/repo/src/lib.rs\"}\n</system-reminder>",
        ),
    ));

    assert_eq!(
        compact_file_reference_chip_path(&message, Some("/repo")).as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(attachment_summary_text(&message), None);
}

#[test]
fn compact_file_reference_chip_path_supports_multiple_attachments() {
    let messages = [
        Message::Attachment(AttachmentMessage::compact_file_reference(
            CompactFileReferencePayload {
                filename: "/repo/Cargo.toml".to_string(),
                display_path: "Cargo.toml".to_string(),
            },
            LlmMessage::user_text(""),
        )),
        Message::Attachment(AttachmentMessage::compact_file_reference(
            CompactFileReferencePayload {
                filename: "/repo/src/lib.rs".to_string(),
                display_path: "src/lib.rs".to_string(),
            },
            LlmMessage::user_text(""),
        )),
    ];

    let paths = messages
        .iter()
        .map(|message| compact_file_reference_chip_path(message, Some("/repo")))
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            Some("Cargo.toml".to_string()),
            Some("src/lib.rs".to_string())
        ]
    );
    assert!(
        messages
            .iter()
            .all(|message| attachment_summary_text(message).is_none())
    );
}

#[test]
fn compact_file_reference_chip_path_supports_legacy_body() {
    let message = Message::Attachment(AttachmentMessage::api(
        AttachmentKind::CompactFileReference,
        LlmMessage::user_text(
            "<system-reminder>\nCalled the Read tool with the following input: {\"file_path\":\"/repo/src/main.rs\"}\nResult of calling the Read tool:\nfn main() {}\n</system-reminder>",
        ),
    ));

    assert_eq!(
        compact_file_reference_chip_path(&message, Some("/repo")).as_deref(),
        Some("src/main.rs")
    );
    assert_eq!(attachment_summary_text(&message), None);
}
