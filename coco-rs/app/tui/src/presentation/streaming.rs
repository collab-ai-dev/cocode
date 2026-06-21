//! Source-backed presentation model for the active streaming tail.

use crate::presentation::thinking::estimate_reasoning_tokens;
use crate::state::ui::StreamingState;

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingTailInput<'a> {
    pub(crate) streaming: &'a StreamingState,
    pub(crate) show_thinking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamingTailView<'a> {
    pub(crate) assistant_text: Option<&'a str>,
    pub(crate) thinking_tokens: Option<i64>,
}

pub(crate) fn streaming_tail_view(input: StreamingTailInput<'_>) -> StreamingTailView<'_> {
    let content = input.streaming.visible_content();
    let assistant_text = (!content.is_empty()).then_some(content);
    let thinking_tokens = (input.show_thinking && !input.streaming.thinking.is_empty())
        .then(|| estimate_reasoning_tokens(&input.streaming.thinking));

    StreamingTailView {
        assistant_text,
        thinking_tokens,
    }
}

#[cfg(test)]
#[path = "streaming.test.rs"]
mod tests;
