use super::*;

impl SessionRuntime {
    pub async fn rewind_files(
        &self,
        request: super::SessionFileRewindRequest,
    ) -> Result<super::SessionFileRewindResult, super::SessionFileRewindError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileRewindError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await;
        let config_home = self.config_home().clone();

        {
            let history = history_arc.read().await;
            if !history.can_restore(&request.user_message_id) {
                return Err(super::SessionFileRewindError::SnapshotMissing(
                    request.user_message_id,
                ));
            }
        }

        let stats = {
            let history = history_arc.read().await;
            history
                .get_diff_stats(&request.user_message_id, &config_home, session_id.as_str())
                .await
                .map_err(|source| super::SessionFileRewindError::Operation {
                    context: if request.dry_run {
                        "file rewind dry run"
                    } else {
                        "file rewind preview"
                    },
                    source: anyhow::Error::from(source),
                })?
        };

        if request.dry_run {
            return Ok(super::SessionFileRewindResult {
                files_changed: stats.files_changed,
                insertions: stats.insertions,
                deletions: stats.deletions,
                dry_run: true,
            });
        }

        let restored = {
            let history = history_arc.read().await;
            history
                .rewind(&request.user_message_id, &config_home, session_id.as_str())
                .await
                .map_err(|source| super::SessionFileRewindError::Operation {
                    context: "file rewind apply",
                    source: anyhow::Error::from(source),
                })?
        };

        Ok(super::SessionFileRewindResult {
            files_changed: restored,
            insertions: stats.insertions,
            deletions: stats.deletions,
            dry_run: false,
        })
    }
    pub async fn render_session_file_diff(
        &self,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileDiffError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await.to_string();
        let config_home = self.config_home().clone();
        let file_history = history_arc.read().await;
        file_history
            .render_session_diff(&config_home, &session_id)
            .await
            .map_err(|source| super::SessionFileDiffError::Operation {
                context: "session file diff",
                source: anyhow::Error::from(source),
            })
    }
    pub async fn rewind_diff_stats(
        &self,
        message_id: &str,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        self.rewind_diff_stats_between(message_id, None).await
    }
    pub async fn rewind_diff_stats_between(
        &self,
        message_id: &str,
        next_message_id: Option<&str>,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileDiffError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await.to_string();
        let config_home = self.config_home().clone();
        let file_history = history_arc.read().await;
        if !file_history.can_restore(message_id) {
            return Ok(None);
        }
        file_history
            .get_diff_stats_between(message_id, next_message_id, &config_home, &session_id)
            .await
            .map(Some)
            .map_err(|source| super::SessionFileDiffError::Operation {
                context: "rewind diff stats",
                source: anyhow::Error::from(source),
            })
    }
    pub async fn render_turn_file_diff(
        &self,
        message_id: &str,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        let Some(history_arc) = self.file_history().cloned() else {
            return Err(super::SessionFileDiffError::NotEnabled);
        };
        let session_id = self.current_typed_session_id().await.to_string();
        let config_home = self.config_home().clone();
        let file_history = history_arc.read().await;
        let Some(next_message_id) = next_file_history_snapshot_id(&file_history, message_id) else {
            return Err(super::SessionFileDiffError::SnapshotMissing(
                message_id.to_string(),
            ));
        };
        file_history
            .render_diff_between(
                message_id,
                next_message_id.as_deref(),
                &config_home,
                &session_id,
            )
            .await
            .map_err(|source| super::SessionFileDiffError::Operation {
                context: "turn file diff",
                source: anyhow::Error::from(source),
            })
    }
    pub async fn pre_clear_rewind_messages(&self) -> Option<Vec<Arc<Message>>> {
        self.engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await
            .clone()
    }
    pub async fn restore_pre_clear_rewind_prefix(
        &self,
        message_id: &str,
    ) -> Option<(i32, i32, Vec<Message>)> {
        let messages = self
            .engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await
            .clone()?;
        let idx = messages.iter().position(|m| match m.as_ref() {
            Message::User(u) => u.uuid.to_string() == message_id,
            _ => false,
        })?;
        let selected_prompt =
            coco_messages::wrapping::extract_text_from_message(messages[idx].as_ref());
        let pre_count = messages.len() as i32;
        let kept: Vec<Message> = messages[..idx]
            .iter()
            .map(|message| message.as_ref().clone())
            .collect();
        let messages_removed = (pre_count - idx as i32).max(0);
        {
            let mut history = self.history_resources.history().lock().await;
            let replacement = kept.iter().cloned().map(Arc::new).collect();
            let no_event_tx = None;
            coco_query::history_sync::history_replace_and_emit(
                &mut history,
                replacement,
                &no_event_tx,
                coco_types::HistoryReplaceReason::Rewind,
            )
            .await;
        }
        self.persist_local_transcript_messages(&kept).await;
        let session_id = self.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        if let Err(e) = self.persistence.transcript_store().append_metadata(
            &session_id_string,
            &coco_session::MetadataEntry::LastPrompt {
                session_id: session_id.clone(),
                last_prompt: selected_prompt.trim().to_string(),
                leaf_uuid: Some(message_id.to_string()),
                explicit: true,
                rewound: true,
            },
        ) {
            warn!(error = %e, session_id = %session_id, message_id, "failed to persist rewind last-prompt metadata");
        }
        Some((idx as i32, messages_removed, kept))
    }
}
