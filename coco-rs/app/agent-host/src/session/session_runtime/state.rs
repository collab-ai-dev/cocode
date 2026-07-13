use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::warn;

use coco_messages::Message;
use coco_query::CommandQueue;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::ToolRegistry;
use coco_types::ProviderModelSelection;
use coco_types::SessionId;
use tokio_util::sync::CancellationToken;

use super::*;

mod accessors;
mod engine_config;
mod file_history;
mod history;
mod integration;
mod lifecycle;
mod metadata;
mod persistence;
mod plan;
mod rewind;
mod side_query;

pub(super) use file_history::TranscriptFileHistorySink;
pub(super) use file_history::file_checkpointing_enabled;

fn next_file_history_snapshot_id(
    file_history: &coco_context::FileHistoryState,
    message_id: &str,
) -> Option<Option<String>> {
    let idx = file_history
        .snapshots
        .iter()
        .position(|snapshot| snapshot.message_id == message_id)?;
    Some(
        file_history
            .snapshots
            .get(idx + 1)
            .map(|snapshot| snapshot.message_id.clone()),
    )
}
