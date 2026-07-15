use super::*;

impl SessionRuntime {
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
