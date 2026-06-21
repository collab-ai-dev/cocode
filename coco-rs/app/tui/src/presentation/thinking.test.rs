use super::*;
use crate::theme::Theme;
use crate::theme::ThemeName;
use pretty_assertions::assert_eq;

#[test]
fn collapsed_thinking_with_content_has_compact_header() {
    let theme = Theme::from_name(ThemeName::Dark);
    let lines = render_thinking_block(
        ThinkingRenderInput {
            content: "inspect files",
            duration_ms: Some(1234),
            reasoning_tokens: Some(220),
            toggle_hint: None,
            display: ThinkingDisplay::Collapsed,
        },
        UiStyles::new(&theme),
    );

    assert_eq!(
        lines[0].spans[0].content.as_ref(),
        "⏺ Thinking · 1.2s · 220 reasoning tok"
    );
}

#[test]
fn collapsed_thinking_appends_toggle_hint() {
    let theme = Theme::from_name(ThemeName::Dark);
    let lines = render_thinking_block(
        ThinkingRenderInput {
            content: "inspect files",
            duration_ms: Some(1234),
            reasoning_tokens: Some(220),
            toggle_hint: Some("F2 to expand"),
            display: ThinkingDisplay::Collapsed,
        },
        UiStyles::new(&theme),
    );

    assert_eq!(
        lines[0].spans[0].content.as_ref(),
        "⏺ Thinking · 1.2s · 220 reasoning tok · F2 to expand"
    );
}

#[test]
fn collapsed_thinking_without_content_has_no_expand_hint() {
    let theme = Theme::from_name(ThemeName::Dark);
    let lines = render_thinking_block(
        ThinkingRenderInput {
            content: "",
            duration_ms: Some(1200),
            reasoning_tokens: Some(1500),
            toggle_hint: None,
            display: ThinkingDisplay::Collapsed,
        },
        UiStyles::new(&theme),
    );

    assert_eq!(
        lines[0].spans[0].content.as_ref(),
        "⏺ Thinking · 1.2s · 1.5k reasoning tok"
    );
}

#[test]
fn expanded_thinking_body_uses_plain_indent() {
    let theme = Theme::from_name(ThemeName::Dark);
    let lines = render_thinking_block(
        ThinkingRenderInput {
            content: "first\nsecond",
            duration_ms: None,
            reasoning_tokens: Some(2),
            toggle_hint: None,
            display: ThinkingDisplay::Expanded {
                max_body_lines: 10,
                truncated_hint: "… truncated",
            },
        },
        UiStyles::new(&theme),
    );

    assert_eq!(lines[1].spans[0].content.as_ref(), "  first");
    assert_eq!(lines[2].spans[0].content.as_ref(), "  second");
}

#[test]
fn expanded_full_thinking_body_has_no_line_cap() {
    let theme = Theme::from_name(ThemeName::Dark);
    let content = (0..8)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = render_thinking_block(
        ThinkingRenderInput {
            content: &content,
            duration_ms: None,
            reasoning_tokens: None,
            toggle_hint: None,
            display: ThinkingDisplay::ExpandedFull,
        },
        UiStyles::new(&theme),
    );

    assert_eq!(lines.len(), 9);
    assert_eq!(lines[8].spans[0].content.as_ref(), "  line-7");
}

#[test]
fn expanded_full_thinking_body_does_not_truncate_long_lines() {
    let theme = Theme::from_name(ThemeName::Dark);
    let content = "x".repeat(crate::presentation::transcript::TRANSCRIPT_LINE_CHAR_CAP + 5);
    let lines = render_thinking_block(
        ThinkingRenderInput {
            content: &content,
            duration_ms: None,
            reasoning_tokens: None,
            toggle_hint: None,
            display: ThinkingDisplay::ExpandedFull,
        },
        UiStyles::new(&theme),
    );

    assert_eq!(lines[1].spans[0].content.as_ref(), format!("  {content}"));
}
