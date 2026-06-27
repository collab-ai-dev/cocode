//! Production [`coco_tool_runtime::AgentTranscriptStore`] backed by
//! [`coco_session::TranscriptStore`].
//!
//! Bridges `coco-coordinator` (root layer, where bg AgentTool
//! spawns register) to `coco-session` (app layer, where transcript
//! files actually live) without a layer-rule violation. The trait
//! lives in `coco-tool-runtime`; this impl wraps the `TranscriptStore`
//! and translates its sync API to the trait's async surface.
//!
//! ## Storage layout
//!
//! For each background agent:
//!
//! ```text
//! <sessions_dir>/<session_id>/subagents/agent-<id>.jsonl    # full message history
//! <sessions_dir>/<session_id>/subagents/agent-<id>.meta.json # AgentSpawnMetadata
//! ```
//!
//! ## Why a separate file from the per-task `.output`
//!
//! `coco_cli::disk_task_output` writes raw text deltas to
//! `<config_home>/cache/tasks/<session>/<task_id>.output` for the
//! `TaskOutput` model-facing tool. That file is text — coco-rs keeps
//! them as separate streams: text for the model's progress view,
//! JSONL for resume. This preserves `TaskOutput`'s text contract
//! while still giving `agent/resume` a clean conversation log to
//! rehydrate from.

use std::sync::Arc;

use async_trait::async_trait;
use coco_session::{AgentMetadata, TranscriptStore};
use coco_tool_runtime::{AgentSpawnMetadata, AgentTranscriptStore};

/// Trait impl for the production transcript store. Cloning is
/// cheap (`Arc<TranscriptStore>` underneath); same instance is
/// shared between `SwarmAgentHandle` (writer side) and the resume
/// entry point (reader side).
pub struct SessionAgentTranscriptStore {
    store: Arc<TranscriptStore>,
}

impl SessionAgentTranscriptStore {
    pub fn new(store: Arc<TranscriptStore>) -> Self {
        Self { store }
    }
}

fn boxed_anyhow(e: anyhow::Error) -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(
        e.to_string(),
        coco_error::StatusCode::Internal,
    ))
}

fn boxed_session_err(e: coco_session::SessionError) -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(
        e.to_string(),
        coco_error::StatusCode::IoError,
    ))
}

#[async_trait]
impl AgentTranscriptStore for SessionAgentTranscriptStore {
    async fn append_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        messages: &[Arc<coco_messages::Message>],
    ) -> Result<(), coco_error::BoxedError> {
        let store = self.store.clone();
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        // Move an owned snapshot into the blocking thread — the
        // Arc-vec clone is cheap pointer bumps; serialisation to
        // bytes happens once inside storage.
        let messages = messages.to_vec();
        tokio::task::spawn_blocking(move || {
            store.append_agent_messages(&session_id, &agent_id, &messages)
        })
        .await
        .map_err(|e| boxed_anyhow(anyhow::anyhow!("spawn_blocking join: {e}")))?
        .map_err(boxed_session_err)
    }

    async fn load_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<Vec<Arc<coco_messages::Message>>>, coco_error::BoxedError> {
        let store = self.store.clone();
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        tokio::task::spawn_blocking(move || store.load_agent_messages(&session_id, &agent_id))
            .await
            .map_err(|e| boxed_anyhow(anyhow::anyhow!("spawn_blocking join: {e}")))?
            .map_err(boxed_session_err)
    }

    async fn write_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
        metadata: &AgentSpawnMetadata,
    ) -> Result<(), coco_error::BoxedError> {
        let store = self.store.clone();
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        let session_meta = AgentMetadata {
            agent_type: metadata.agent_type.clone(),
            worktree_path: metadata.worktree_path.clone(),
            description: metadata.description.clone(),
        };
        tokio::task::spawn_blocking(move || {
            store.write_agent_metadata(&session_id, &agent_id, &session_meta)
        })
        .await
        .map_err(|e| boxed_anyhow(anyhow::anyhow!("spawn_blocking join: {e}")))?
        .map_err(boxed_session_err)
    }

    async fn read_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<AgentSpawnMetadata>, coco_error::BoxedError> {
        let store = self.store.clone();
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        let session_meta: Option<AgentMetadata> =
            tokio::task::spawn_blocking(move || store.read_agent_metadata(&session_id, &agent_id))
                .await
                .map_err(|e| boxed_anyhow(anyhow::anyhow!("spawn_blocking join: {e}")))?
                .map_err(boxed_session_err)?;
        Ok(session_meta.map(|m| AgentSpawnMetadata {
            agent_type: m.agent_type,
            worktree_path: m.worktree_path,
            description: m.description,
        }))
    }
}

#[cfg(test)]
#[path = "agent_transcript_persistence.test.rs"]
mod tests;
