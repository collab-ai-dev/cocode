use super::*;
use pretty_assertions::assert_eq;

#[test]
fn streaming_tail_view_uses_visible_content_as_assistant_text() {
    let mut streaming = StreamingState::new();
    streaming.append_text("first\nsecond");
    streaming.advance_display();

    let view = streaming_tail_view(StreamingTailInput {
        streaming: &streaming,
        show_thinking: true,
    });

    assert_eq!(view.assistant_text, Some("first\n"));
    assert_eq!(view.thinking_tokens, None);
}

#[test]
fn streaming_tail_view_adds_thinking_tokens_when_enabled() {
    let mut streaming = StreamingState::new();
    streaming.append_thinking("one two three four");

    let view = streaming_tail_view(StreamingTailInput {
        streaming: &streaming,
        show_thinking: true,
    });

    assert_eq!(view.assistant_text, None);
    assert_eq!(view.thinking_tokens, Some(5));
}

#[test]
fn streaming_tail_view_hides_thinking_when_disabled() {
    let mut streaming = StreamingState::new();
    streaming.append_text("visible");
    streaming.reveal_all();
    streaming.append_thinking("hidden thinking");

    let view = streaming_tail_view(StreamingTailInput {
        streaming: &streaming,
        show_thinking: false,
    });

    assert_eq!(view.assistant_text, Some("visible"));
    assert_eq!(view.thinking_tokens, None);
}

#[test]
fn streaming_tail_view_is_empty_without_visible_content_or_thinking() {
    let streaming = StreamingState::new();

    let view = streaming_tail_view(StreamingTailInput {
        streaming: &streaming,
        show_thinking: true,
    });

    assert_eq!(view.assistant_text, None);
    assert_eq!(view.thinking_tokens, None);
}
