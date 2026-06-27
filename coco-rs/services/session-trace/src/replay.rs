//! Offline replay: read a bundle back into a [`ReplayBundle`] with reduced
//! per-call status and a compaction count, for golden assertions.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::IoSnafu;
use crate::error::MalformedSnafu;
use crate::error::Result;
use crate::error::SerdeSnafu;
use crate::event::TraceEvent;
use crate::writer::EVENTS_FILE;
use crate::writer::MANIFEST_FILE;
use crate::writer::SCHEMA_VERSION;
use crate::writer::TraceManifest;
use crate::writer::TraceRecord;

/// Reduced lifecycle state of a single tool call across the trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallStatus {
    Queued,
    Started,
    Completed { is_error: bool },
}

/// A replayed bundle: the ordered events plus derived summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayBundle {
    pub manifest: TraceManifest,
    /// Events in `seq` order.
    pub events: Vec<TraceEvent>,
    /// Final lifecycle status per builtin-tool `call_id`.
    pub tool_calls: BTreeMap<String, ToolCallStatus>,
    /// Number of `ContextCompacted` edges.
    pub compaction_count: usize,
}

/// Read and reduce the bundle at `dir`. Records are sorted by `seq` so the
/// replay is deterministic regardless of on-disk line order.
pub fn replay_bundle(dir: impl AsRef<Path>) -> Result<ReplayBundle> {
    let dir = dir.as_ref();

    let manifest_raw = std::fs::read_to_string(dir.join(MANIFEST_FILE)).map_err(|e| {
        IoSnafu {
            message: format!("read {MANIFEST_FILE}: {e}"),
        }
        .build()
    })?;
    let manifest: TraceManifest = serde_json::from_str(&manifest_raw).map_err(|e| {
        SerdeSnafu {
            message: format!("manifest: {e}"),
        }
        .build()
    })?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(MalformedSnafu {
            message: format!(
                "unsupported schema_version {} (expected {SCHEMA_VERSION})",
                manifest.schema_version
            ),
        }
        .build());
    }

    let events_raw = std::fs::read_to_string(dir.join(EVENTS_FILE)).map_err(|e| {
        IoSnafu {
            message: format!("read {EVENTS_FILE}: {e}"),
        }
        .build()
    })?;
    let mut records: Vec<TraceRecord> = Vec::new();
    for (i, line) in events_raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: TraceRecord = serde_json::from_str(line).map_err(|e| {
            MalformedSnafu {
                message: format!("{EVENTS_FILE} line {}: {e}", i + 1),
            }
            .build()
        })?;
        records.push(record);
    }
    records.sort_by_key(|r| r.seq);
    let events: Vec<TraceEvent> = records.into_iter().map(|r| r.event).collect();

    let mut tool_calls: BTreeMap<String, ToolCallStatus> = BTreeMap::new();
    let mut compaction_count = 0usize;
    for event in &events {
        match event {
            TraceEvent::ToolQueued { call_id, .. } => {
                tool_calls
                    .entry(call_id.clone())
                    .or_insert(ToolCallStatus::Queued);
            }
            TraceEvent::ToolStarted { call_id, .. } => {
                tool_calls.insert(call_id.clone(), ToolCallStatus::Started);
            }
            TraceEvent::ToolCompleted {
                call_id, is_error, ..
            } => {
                tool_calls.insert(
                    call_id.clone(),
                    ToolCallStatus::Completed {
                        is_error: *is_error,
                    },
                );
            }
            TraceEvent::ContextCompacted => compaction_count += 1,
            // MCP calls and turn/compaction-edge markers are kept in `events`
            // but not reduced into the builtin-tool status map.
            TraceEvent::TurnStarted { .. }
            | TraceEvent::TurnEnded { .. }
            | TraceEvent::McpToolBegin { .. }
            | TraceEvent::McpToolEnd { .. }
            | TraceEvent::CompactionStarted
            | TraceEvent::CompactionFailed => {}
        }
    }

    Ok(ReplayBundle {
        manifest,
        events,
        tool_calls,
        compaction_count,
    })
}

#[cfg(test)]
#[path = "replay.test.rs"]
mod tests;
