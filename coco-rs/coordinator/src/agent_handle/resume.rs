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
use coco_types::SessionId;

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
        parent_session_id: SessionId,
    ) -> Result<AgentSpawnResponse, String> {
        let session_id = parent_session_id.to_string();
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

        if let Some(killed_by) = meta.killed_by {
            return Err(format!(
                "Agent {original_agent_id} was stopped by {} and will not be resumed. Spawn a fresh agent instead.",
                killed_by.as_str()
            ));
        }

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

        // Worktree revival: reuse the original worktree if it still exists;
        // otherwise, if the agent was worktree-isolated, re-isolate (its
        // worktree is typically GC'd on completion) so the resumed run stays
        // isolated. Fall back to the parent cwd only when neither applies.
        // Mirrors TS resumeAgent's worktree handling.
        let worktree_alive = meta
            .worktree_path
            .as_deref()
            .is_some_and(|p| std::path::Path::new(p).is_dir());
        let (cwd_override, isolation) = if worktree_alive {
            (
                meta.worktree_path.as_deref().map(std::path::PathBuf::from),
                None,
            )
        } else if meta.isolation == Some(coco_types::AgentIsolation::Worktree) {
            (None, Some(coco_types::AgentIsolation::Worktree))
        } else {
            (None, None)
        };

        // Inherit the session's current features (coordinator-mode etc.) so the
        // resumed agent runs in the live session context, like a fresh spawn.
        let features = Some(std::sync::Arc::new(self.runtime_config().features.clone()));

        // Restore the agent's definition (system prompt, tool restrictions,
        // model, effort, hooks, mcp, skills) by re-resolving its persisted
        // `agent_type` against the live catalog — mirroring a fresh AgentTool
        // spawn. Without this the resumed agent would run with a generic
        // identity and no frontmatter tool restrictions.
        let definition = self.resolve_agent_definition(&meta.agent_type).await;

        let resume_request = AgentSpawnRequest {
            prompt,
            description: meta
                .description
                .clone()
                .or_else(|| Some("(resumed)".into())),
            subagent_type: Some(meta.agent_type.clone()),
            definition,
            // Restore the spawn-time permission mode (Plan etc.) so the resumed
            // agent doesn't silently drop to Default.
            mode: meta.mode,
            isolation,
            features,
            run_in_background: true,
            cwd: cwd_override,
            session_id: Some(parent_session_id.clone()),
            // `Resume` (not `Fork`) — the child engine sees the persisted
            // history as its starting point but builds a fresh system
            // prompt from the agent definition (Fork instead inherits the
            // parent's pre-rendered prompt verbatim for cache parity).
            spawn_mode: coco_tool_runtime::SpawnMode::Resume {
                parent_messages: filtered,
                // Reuse the original id so the resumed run's transcript,
                // content-replacement records, and metadata stay in the same
                // per-agent files (continuity), mirroring TS.
                resumed_agent_id: original_agent_id.to_string(),
            },
            ..Default::default()
        };

        self.spawn_subagent(&resume_request).await
    }
}
