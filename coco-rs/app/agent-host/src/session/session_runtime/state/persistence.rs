use super::*;

impl SessionRuntime {
    /// Apply AppServer `control/updateEnv` updates to the session-scoped shell environment.
    ///
    /// Empty values mean "unset", matching the remote
    /// `control/updateEnv` convention. When shell tools are disabled there is
    /// no consumer, but we still count the accepted update for telemetry and
    /// protocol compatibility.
    pub fn apply_session_env_updates(
        &self,
        env: std::collections::HashMap<String, String>,
    ) -> (i32, i32) {
        let mut applied = 0_i32;
        let mut cleared = 0_i32;
        let session_env_vars = self.engine_state_resources.session_env_vars();

        for (key, value) in env {
            if value.is_empty() {
                if let Some(store) = session_env_vars {
                    store.delete(&key);
                }
                cleared += 1;
            } else {
                if let Some(store) = session_env_vars {
                    store.set(key, value);
                }
                applied += 1;
            }
        }

        (applied, cleared)
    }
    pub fn session_env_snapshot(&self) -> Option<std::collections::HashMap<String, String>> {
        self.engine_state_resources
            .session_env_vars()
            .map(coco_shell::SessionEnvVars::snapshot)
    }
    pub async fn prompt_history_texts(&self, project: String) -> Vec<String> {
        let config_home = self.config_home().clone();
        let session_id = self.current_typed_session_id().await;
        tokio::task::spawn_blocking(move || {
            coco_session::PromptHistory::new(&config_home, &project, &session_id)
                .get_history()
                .into_iter()
                .map(|entry| entry.display)
                .collect()
        })
        .await
        .unwrap_or_default()
    }
    pub async fn persist_prompt_history_entry(
        &self,
        project: String,
        display: String,
    ) -> anyhow::Result<()> {
        let config_home = self.config_home().clone();
        let session_id = self.current_typed_session_id().await;
        tokio::task::spawn_blocking(move || {
            coco_session::PromptHistory::new(&config_home, &project, &session_id)
                .add(&display)
                .map_err(anyhow::Error::from)
        })
        .await?
    }
    pub async fn persist_local_transcript_messages(&self, messages: &[coco_messages::Message]) {
        if !self.persistence.persist_session() || messages.is_empty() {
            return;
        }
        let session_id = self.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        let store = Arc::clone(self.persistence.transcript_store());
        let seen = Arc::clone(self.engine_state_resources.transcript_dedup());
        let cwd_path = self.current_cwd().read().await.clone();
        let cwd = cwd_path.display().to_string();
        let git_branch = coco_git::get_current_branch(&cwd_path)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty());
        let now = chrono::Utc::now().to_rfc3339();
        let message_owned = messages.to_vec();
        let mut seen_guard = seen.lock().await;
        let message_refs: Vec<&coco_messages::Message> = message_owned.iter().collect();
        let options = coco_session::storage::ChainWriteOptions {
            cwd,
            timestamp: now,
            is_sidechain: false,
            agent_id: None,
            starting_parent_uuid: None,
            git_branch,
        };
        if let Err(e) =
            store.append_message_chain(&session_id_string, &message_refs, &mut seen_guard, options)
        {
            warn!(error = %e, session_id = %session_id, "failed to persist local transcript messages");
        }
    }
    pub async fn list_persisted_session_summaries(
        &self,
    ) -> anyhow::Result<coco_types::SessionListResult> {
        let manager = Arc::clone(self.session_manager());
        tokio::task::spawn_blocking(move || {
            let sessions = manager.list()?;
            let summaries = sessions
                .into_iter()
                .map(|session| {
                    Ok(coco_types::SessionSummary {
                        session_id: coco_types::SessionId::try_new(session.id)?,
                        model: session.model,
                        cwd: session.working_dir.to_string_lossy().into_owned(),
                        created_at: session.created_at,
                        updated_at: session.updated_at,
                        title: session.title,
                        message_count: session.message_count,
                        total_tokens: session.total_tokens,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(coco_types::SessionListResult {
                sessions: summaries,
            })
        })
        .await?
    }
    /// Seed the transcript dedup set with uuids that are already
    /// persisted on disk. Called on resume / fork so the first
    /// post-load turn doesn't re-write the loaded messages.
    /// MUST clear the dedup set first. In-TUI `/resume` reuses the
    /// runtime, so without the clear the prior session's UUIDs leak
    /// into the new session and any colliding new write gets silently
    /// suppressed.
    pub(crate) async fn seed_transcript_dedup<I>(&self, uuids: I)
    where
        I: IntoIterator<Item = uuid::Uuid>,
    {
        let mut g = self.engine_state_resources.transcript_dedup().lock().await;
        g.clear();
        g.extend(uuids);
    }
    /// Reconstruct Level 2 tool-result replacement state from the
    /// restored messages plus transcript content-replacement records.
    /// Called on resume/fork before the first resumed turn.
    /// `agent_id` MUST be the runtime's current agent_id (None for
    /// main-thread sessions, Some for subagents). The transcript
    /// content-replacement records are stamped with `agent_id` at
    /// write time (`engine_prompt.rs:200-216`); reading with
    /// `agent_id: None` for a subagent resume would silently drop
    /// every Level-2 replacement and force the model to re-read the
    /// full tool result, breaking prompt-cache stability.
    pub(crate) async fn seed_tool_result_replacement_state(
        &self,
        messages: &[Message],
        session_id: &SessionId,
        agent_id: Option<&str>,
    ) {
        let records = self
            .persistence
            .transcript_store()
            .load_content_replacements_for_chain(session_id.as_str(), agent_id)
            .unwrap_or_default();
        let mut next =
            coco_tool_runtime::tool_result_offload::ContentReplacementState::new(i64::MAX);
        for msg in messages {
            if let Message::ToolResult(tr) = msg {
                next.seen_ids.insert(tr.tool_use_id.clone());
            }
        }
        for record in records {
            next.seen_ids.insert(record.tool_use_id().to_string());
            next.replacements.insert(
                record.tool_use_id().to_string(),
                record.replacement().to_string(),
            );
        }
        *self
            .engine_state_resources
            .tool_result_replacement_state()
            .write()
            .await = next;
    }
}
