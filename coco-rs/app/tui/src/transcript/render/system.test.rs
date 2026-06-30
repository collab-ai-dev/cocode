use super::compact_boundary_text;
use super::memory_saved_summary;
use coco_messages::Message;
use coco_messages::SystemMemorySavedMessage;
use coco_messages::SystemMessage;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;
use std::sync::Arc;
use uuid::Uuid;

fn flatten(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect()
}

/// The compact-boundary line must interpolate the resolved shortcut
/// verbatim — independent of which locale (`en`, `zh-CN`, …) happens
/// to be active when the test runs. We only assert the
/// locale-independent contract: the shortcut argument appears in the
/// rendered string. The localized prefix ("Conversation compacted" /
/// "对话已压缩") is exercised by the i18n snapshot tests.
#[test]
fn compact_boundary_text_interpolates_the_shortcut() {
    let text = compact_boundary_text("ctrl+o");
    assert!(text.contains("ctrl+o"));
    assert!(!text.is_empty());
}

#[test]
fn memory_saved_summary_uses_verb_and_count() {
    let saved = SystemMemorySavedMessage {
        uuid: Uuid::nil(),
        written_paths: vec!["a.md".to_string(), "b.md".to_string()],
        verb: "Saved".to_string(),
    };
    assert_eq!(memory_saved_summary(&saved), "Saved 2 memories");

    let improved = SystemMemorySavedMessage {
        uuid: Uuid::nil(),
        written_paths: vec!["a.md".to_string()],
        verb: "Improved".to_string(),
    };
    assert_eq!(memory_saved_summary(&improved), "Improved 1 memory");
}

#[test]
fn memory_saved_render_shows_summary_without_paths() {
    let msg = Message::System(SystemMessage::MemorySaved(SystemMemorySavedMessage {
        uuid: Uuid::nil(),
        written_paths: vec!["/mem/a.md".to_string(), "/mem/b.md".to_string()],
        verb: "Saved".to_string(),
    }));
    let cell = crate::transcript::cells::RenderedCell {
        message_uuid: Uuid::nil(),
        kind: crate::transcript::cells::CellKind::System(
            crate::transcript::cells::SystemCellKind::MemorySaved,
        ),
        source: Arc::new(msg),
    };
    let theme = Theme::default();
    let cells = Vec::new();
    let renderer = crate::transcript::render::CellsRenderer::new(&cells, UiStyles::new(&theme));
    let mut lines = Vec::new();

    super::try_render(&renderer, &cell, &mut lines).expect("system cell renders");

    let text = flatten(&lines);
    assert!(text.contains("Saved 2 memories"), "{text}");
    assert!(!text.contains("/mem/a.md"), "{text}");
    assert!(!text.contains("/mem/b.md"), "{text}");
}
