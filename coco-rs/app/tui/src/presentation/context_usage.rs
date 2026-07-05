use std::collections::HashSet;

use coco_types::ModelRole;

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderContextUsage {
    pub used: i64,
    pub total: i64,
    pub percent: i64,
    pub percent_tenths: i64,
}

/// The most recent point in history whose exact token footprint is known —
/// the baseline the tail estimate is added to.
#[derive(Debug, Clone, Copy)]
enum ContextAnchor {
    /// An assistant message's reported cumulative `usage.total()`.
    Assistant(i64),
    /// A compaction boundary's post-compact context size (`tokens_after`).
    /// Keeps `ctx %` meaningful right after a `/compact`, where no assistant
    /// usage anchor survives the rewrite.
    CompactBoundary(i64),
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
    let mut anchor: Option<(usize, ContextAnchor)> = None;
    for (idx, msg) in messages.iter().enumerate() {
        match msg.as_ref() {
            coco_messages::Message::Assistant(assistant) => {
                if let Some(usage) = assistant.usage {
                    anchor = Some((idx, ContextAnchor::Assistant(usage.total())));
                }
            }
            coco_messages::Message::System(coco_messages::SystemMessage::CompactBoundary(
                boundary,
            )) => {
                anchor = Some((idx, ContextAnchor::CompactBoundary(boundary.tokens_after)));
            }
            _ => {}
        }
    }
    let (idx, anchor) = anchor?;
    // `tokens_after` already counts the compact-summary message that
    // immediately follows the boundary, so the tail estimate must skip it to
    // avoid double-counting; an assistant anchor counts everything after it.
    let (baseline, tail_start) = match anchor {
        ContextAnchor::Assistant(total) => (total, idx + 1),
        ContextAnchor::CompactBoundary(tokens_after) => {
            let mut start = idx + 1;
            if messages.get(start).is_some_and(
                |m| matches!(m.as_ref(), coco_messages::Message::User(u) if u.is_compact_summary),
            ) {
                start += 1;
            }
            (tokens_after, start)
        }
    };
    let tail_tokens = coco_messages::estimate_tokens_for_messages(&messages[tail_start..]);
    let used = baseline.saturating_add(tail_tokens);
    let total = total.max(1);
    Some(RenderContextUsage {
        used,
        total,
        percent: (used.saturating_mul(100) / total).clamp(0, 100),
        percent_tenths: (used.saturating_mul(1000).saturating_add(total / 2) / total)
            .clamp(0, 1000),
    })
}

#[cfg(test)]
#[path = "context_usage.test.rs"]
mod tests;
