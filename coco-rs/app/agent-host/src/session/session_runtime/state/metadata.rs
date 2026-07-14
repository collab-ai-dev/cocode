use super::*;

impl SessionRuntime {
    pub async fn active_goal_snapshot(&self) -> Option<coco_types::ActiveGoal> {
        self.engine_state_resources
            .app_state()
            .read()
            .await
            .active_goal
            .clone()
    }
    pub async fn restore_goal_from_history(
        &self,
        messages: &[Arc<coco_messages::Message>],
        trust_rejected: bool,
    ) -> Option<coco_types::ActiveGoal> {
        let cfg = self.current_engine_config().await;
        let goal = crate::goal_command::restore_goal_from_history(
            messages,
            self.engine_state_resources.app_state(),
            self.hook_resources.registry().as_ref(),
            self.session_usage_snapshot().await.totals.output_tokens,
            crate::goal_command::GoalGate {
                hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
                trust_rejected,
            },
        )
        .await;
        self.persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
        goal
    }
    pub async fn persist_goal_metadata(&self, goal: Option<coco_session::GoalMetadata>) {
        if !self.persistence.persist_session() {
            return;
        }
        self.engine_state_resources
            .terminal_goal_metadata_written()
            .store(goal.as_ref().is_some_and(|goal| goal.met), Ordering::SeqCst);
        let session_id = self.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        let store = Arc::clone(self.persistence.transcript_store());
        let entry = coco_session::MetadataEntry::Goal {
            session_id: session_id.clone(),
            goal,
        };
        let session_id_for_write = session_id_string;
        match tokio::task::spawn_blocking(move || {
            store.append_metadata(&session_id_for_write, &entry)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(error = %e, session_id = %session_id, "failed to persist goal metadata");
            }
            Err(e) => {
                warn!(error = %e, session_id = %session_id, "goal metadata write task failed");
            }
        }
    }
    pub async fn re_append_session_metadata(&self) {
        let session_id = self.current_typed_session_id().await.to_string();
        let manager = Arc::clone(self.session_manager());
        let _ =
            tokio::task::spawn_blocking(move || manager.re_append_session_metadata(&session_id))
                .await;
    }
    pub async fn has_persisted_title(&self) -> bool {
        let session_id = self.current_typed_session_id().await.to_string();
        let manager = Arc::clone(self.session_manager());
        tokio::task::spawn_blocking(move || {
            manager
                .load(&session_id)
                .map(|session| session.title.is_some())
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }
    pub async fn persist_session_title(&self, name: String) -> anyhow::Result<()> {
        let session_id = self.current_typed_session_id().await.to_string();
        let manager = Arc::clone(self.session_manager());
        tokio::task::spawn_blocking(move || manager.set_title(&session_id, &name))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|inner| inner.map_err(anyhow::Error::from))
            .map(|_| ())
    }
    pub async fn title_generation_conversation_text(&self) -> String {
        let history = self.history_resources.history().lock().await;
        coco_session::title_generator::extract_conversation_text(history.as_slice())
    }
    pub async fn persist_session_mode(&self) {
        let session_id = self.current_typed_session_id().await;
        let manager = Arc::clone(self.session_manager());
        let features = self.runtime_config().features.clone();
        let _ = tokio::task::spawn_blocking(move || {
            crate::coordinator_mode_resume::persist_session_mode(
                manager.as_ref(),
                &session_id,
                &features,
            )
        })
        .await;
    }
    pub fn reconcile_session_mode_on_resume(
        &self,
        stored_mode: Option<&str>,
    ) -> Option<&'static str> {
        crate::coordinator_mode_resume::reconcile_on_resume(
            stored_mode,
            &self.runtime_config().features,
        )
    }
    pub async fn toggle_tag(&self, tag: String) -> anyhow::Result<(SessionId, bool)> {
        let session_id = self.current_typed_session_id().await;
        let session_id_for_toggle = session_id.to_string();
        let manager = Arc::clone(self.session_manager());
        let (_, added) =
            tokio::task::spawn_blocking(move || manager.toggle_tag(&session_id_for_toggle, &tag))
                .await
                .map_err(anyhow::Error::from)?
                .map_err(anyhow::Error::from)?;
        Ok((session_id, added))
    }
}
