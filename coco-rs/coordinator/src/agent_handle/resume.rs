//! Background AgentTool spawn resume.
//!
//! Reads the per-agent JSONL transcript + `meta.json` sidecar,
//! reconstructs `fork_context_messages` from the persisted history,
//! and dispatches a fresh background spawn that picks up where the
//! original left off.
//!
//! Only the conversation history is recoverable — not the streaming
//! connection. The resumed spawn gets a NEW `agent_id` / `task_id`;
//! the model sees an `AsyncLaunched` response just like a fresh spawn.

use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;

use super::SwarmAgentHandle;

impl SwarmAgentHandle {
    /// Resume a previously-completed background AgentTool spawn.
    ///
    /// Required wiring: `set_transcript_store` must have been called at
    /// session bootstrap. Without it, returns
    /// `Err("transcript store not configured")`.
    ///
    /// Returns `Err` when no metadata exists for `original_agent_id`
    /// (never spawned). Missing transcript is non-fatal — a completed
    /// agent that lost its output dir still resumes, it just runs from
    /// the prompt with no prior history.
    pub async fn resume_agent(
        &self,
        original_agent_id: &str,
        prompt: String,
        session_id: String,
    ) -> Result<AgentSpawnResponse, String> {
        let Some(store) = self.transcript_store().cloned() else {
            return Err(
                "Resume requires AgentTranscriptStore: install via SwarmAgentHandle::set_transcript_store at session bootstrap"
                    .into(),
            );
        };

        // Missing meta is fatal because we can't route the resume without
        // `agent_type`.
        let meta = store
            .read_agent_metadata(&session_id, original_agent_id)
            .await
            .map_err(|e| format!("read agent metadata: {e}"))?
            .ok_or_else(|| {
                format!("No metadata found for agent {original_agent_id} in session {session_id}")
            })?;

        let prior_messages = store
            .load_agent_messages(&session_id, original_agent_id)
            .await
            .map_err(|e| format!("load agent transcript: {e}"))?
            .unwrap_or_default();

        // Strip unresolved tool uses + orphaned thinking + whitespace-only
        // assistant messages so the resumed spawn doesn't trip on a partial
        // conversation. Storage now hands back typed `Arc<Message>`, so
        // the filter pass walks the same Arcs the engine will see — no
        // Value → Message round-trip at this seam.
        let filtered = coco_subagent::filter_transcript(&prior_messages);

        // If the worktree directory was removed out from under us, fall
        // back to the parent's cwd.
        let cwd_override = match meta.worktree_path.as_deref() {
            Some(path) if std::path::Path::new(path).is_dir() => {
                Some(std::path::PathBuf::from(path))
            }
            _ => None,
        };

        let resume_request = AgentSpawnRequest {
            prompt,
            description: meta
                .description
                .clone()
                .or_else(|| Some("(resumed)".into())),
            subagent_type: Some(meta.agent_type.clone()),
            run_in_background: true,
            cwd: cwd_override,
            session_id,
            // `Resume` (not `Fork`) — the child engine sees the persisted
            // history as its starting point but builds a fresh system
            // prompt from the agent definition (Fork instead inherits the
            // parent's pre-rendered prompt verbatim for cache parity).
            spawn_mode: coco_tool_runtime::SpawnMode::Resume {
                parent_messages: filtered,
            },
            ..Default::default()
        };

        self.spawn_subagent(&resume_request).await
    }
}
