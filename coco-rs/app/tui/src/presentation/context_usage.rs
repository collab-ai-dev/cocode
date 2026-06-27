use std::collections::HashSet;

use coco_types::ModelRole;

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderContextUsage {
    pub used: i64,
    pub total: i64,
    pub percent: i64,
}

pub(crate) fn render_context_usage(state: &AppState) -> Option<RenderContextUsage> {
    let total = state
        .session
        .model_by_role
        .get(&ModelRole::Main)
        .and_then(|binding| binding.context_window)
        .filter(|tokens| *tokens > 0)?;
    let mut seen = HashSet::new();
    let mut messages = Vec::new();
    for cell in state.session.transcript.cells() {
        if seen.insert(cell.message_uuid) {
            messages.push(cell.source.clone());
        }
    }
    let mut latest: Option<(usize, coco_types::TokenUsage)> = None;
    for (idx, msg) in messages.iter().enumerate() {
        if let coco_messages::Message::Assistant(assistant) = msg.as_ref()
            && let Some(usage) = assistant.usage
        {
            latest = Some((idx, usage));
        }
    }
    let (idx, usage) = latest?;
    let tail_tokens = coco_messages::estimate_tokens_for_messages(&messages[idx + 1..]);
    let used = usage.total().saturating_add(tail_tokens);
    Some(RenderContextUsage {
        used,
        total,
        percent: (used * 100 / total.max(1)).clamp(0, 100),
    })
}
