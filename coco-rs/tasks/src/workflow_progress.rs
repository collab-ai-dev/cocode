//! The workflow progress reducer: folds `agent()`/`phase()`/`log()` deltas into
//! the bounded node array carried on a `LocalWorkflow` row.
//!
//! Two properties the naive "push every delta" shape does not have, and both
//! matter once a run fans out:
//!
//! - **Agent and phase nodes are upserted by index, not appended.** One
//!   `agent()` call emits `start` and then `done`; appending would render the
//!   same agent twice and make any count derived from the array wrong. Keying on
//!   `(kind, index)` collapses every frame for one call onto a single node whose
//!   latest emit is the whole truth.
//! - **Only logs grow, and they are trimmed.** Agent and phase nodes are bounded
//!   by the lifetime `agent()` cap; `log()` is not — a script can call it in a
//!   loop. Without a trim the array, which is cloned into a protocol event on
//!   every delta, grows without bound.

use coco_types::WorkflowProgressEvent;

/// Log-node high-water mark. The trim only fires above `2 ×` this and cuts back
/// to it, so a chatty script pays the O(n) scan roughly every
/// [`MAX_WORKFLOW_PROGRESS_NODES`] logs instead of on every delta.
pub const MAX_WORKFLOW_PROGRESS_NODES: usize = 500;

/// Identity of an upsertable node: agent and phase nodes are keyed by kind +
/// index; logs have no index and are append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKey {
    Agent(i32),
    Phase(i32),
}

fn node_key(event: &WorkflowProgressEvent) -> Option<NodeKey> {
    match event {
        WorkflowProgressEvent::WorkflowAgent { index, .. } => Some(NodeKey::Agent(*index)),
        WorkflowProgressEvent::WorkflowPhase { index, .. } => Some(NodeKey::Phase(*index)),
        WorkflowProgressEvent::WorkflowLog { .. } => None,
    }
}

/// Fold one delta into `nodes`.
///
/// An agent/phase delta replaces the node with the same key wholesale (every
/// emit carries the full field set, so the newest frame is authoritative) or is
/// appended when new. A log delta is appended and may trigger the trim, which
/// drops the **oldest** log nodes and leaves agent/phase nodes and their
/// relative order untouched.
pub fn apply_workflow_progress(
    nodes: &mut Vec<WorkflowProgressEvent>,
    event: WorkflowProgressEvent,
) {
    let Some(key) = node_key(&event) else {
        nodes.push(event);
        trim_logs(nodes);
        return;
    };
    match nodes.iter_mut().find(|node| node_key(node) == Some(key)) {
        Some(existing) => *existing = event,
        None => nodes.push(event),
    }
}

/// Drop the oldest log nodes once the array exceeds `2 ×` the high-water mark,
/// bringing it back down to the mark. Agent and phase nodes are never evicted:
/// they are the run's structure, and losing one would silently change every
/// count derived from the array.
fn trim_logs(nodes: &mut Vec<WorkflowProgressEvent>) {
    if nodes.len() <= MAX_WORKFLOW_PROGRESS_NODES * 2 {
        return;
    }
    let mut to_drop = nodes.len() - MAX_WORKFLOW_PROGRESS_NODES;
    nodes.retain(|node| {
        if to_drop > 0 && matches!(node, WorkflowProgressEvent::WorkflowLog { .. }) {
            to_drop -= 1;
            return false;
        }
        true
    });
}

/// Stamp the host's wall clock onto an agent delta so consumers can order
/// frames that the index-keyed upsert has collapsed in place. Phase and log
/// nodes carry no timestamp.
pub fn stamp_progress_time(event: &mut WorkflowProgressEvent, now_ms: i64) {
    if let WorkflowProgressEvent::WorkflowAgent {
        last_progress_at, ..
    } = event
    {
        *last_progress_at = Some(now_ms);
    }
}

#[cfg(test)]
#[path = "workflow_progress.test.rs"]
mod tests;
