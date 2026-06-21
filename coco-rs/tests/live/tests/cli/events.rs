//! Helpers for asserting on the `CoreEvent` stream emitted by `QueryEngine`.
//!
//! Three event layers exist:
//! - `Protocol(ServerNotification)` — wire-tagged session/turn/context lifecycle
//! - `Stream(AgentStreamEvent)` — content + tool execution deltas
//! - `Tui(TuiOnlyEvent)` — terminal-only signals (irrelevant to live tests)
//!
//! Tests use these helpers instead of inlining `matches!` patterns so a
//! later refactor of the event taxonomy stays mechanical.

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;

/// Names of every tool that started executing during the session.
/// Includes duplicates when a tool runs multiple times.
pub fn tool_uses_started(events: &[CoreEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Stream(AgentStreamEvent::ToolUseStarted { name, .. }) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// Pairs of (tool_name, is_error) for every tool execution that completed.
pub fn tool_uses_completed(events: &[CoreEvent]) -> Vec<(&str, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Stream(AgentStreamEvent::ToolUseCompleted { name, is_error, .. }) => {
                Some((name.as_str(), *is_error))
            }
            _ => None,
        })
        .collect()
}

/// Number of `turn/ended` notifications with `outcome.kind == "completed"`.
pub fn turns_completed(events: &[CoreEvent]) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                CoreEvent::Protocol(ServerNotification::TurnEnded(p))
                    if matches!(p.outcome, coco_types::TurnOutcome::Completed(_))
            )
        })
        .count()
}

/// `true` when at least one compaction event fired.
pub fn saw_compaction(events: &[CoreEvent]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            CoreEvent::Protocol(ServerNotification::CompactionStarted)
                | CoreEvent::Protocol(ServerNotification::ContextCompacted(_))
                | CoreEvent::Protocol(ServerNotification::CompactionPhase(_))
        )
    })
}

/// `true` when the final `SessionResult` reports a clean (non-error) outcome.
pub fn session_succeeded(events: &[CoreEvent]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            CoreEvent::Protocol(ServerNotification::SessionResult(p)) => Some(!p.is_error),
            _ => None,
        })
        .unwrap_or(false)
}

/// Variant tag (without payload) for one event — used to summarize
/// stream contents without printing huge text deltas.
fn variant_tag(event: &CoreEvent) -> String {
    match event {
        CoreEvent::Protocol(n) => format!("Protocol::{:?}", std::mem::discriminant(n)),
        CoreEvent::Stream(s) => format!("Stream::{:?}", std::mem::discriminant(s)),
        CoreEvent::Tui(_) => "Tui".to_string(),
    }
}

/// Notification-method strings (e.g. `turn/completed`, `session/started`)
/// for every Protocol-layer event in the stream. Useful to see what the
/// engine actually emitted when an assertion fails.
pub fn protocol_methods(events: &[CoreEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Protocol(n) => Some(format!("{:?}", n.method())),
            _ => None,
        })
        .collect()
}

/// Concise debug summary of an event stream — useful in assertion
/// failure messages so the user can see what the engine actually
/// emitted without dumping every variant.
pub fn summarize(events: &[CoreEvent]) -> String {
    let total = events.len();
    let started = tool_uses_started(events);
    let completed = tool_uses_completed(events);
    let turns = turns_completed(events);
    let compacted = saw_compaction(events);
    let protocols = protocol_methods(events);
    let _ = variant_tag; // silence unused
    format!(
        "events={total} turns_completed={turns} compaction={compacted} \
         tool_starts={started:?} tool_completions={completed:?} \
         protocols={protocols:?}"
    )
}
