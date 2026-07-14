use super::*;

impl SessionHandle {
    pub async fn persist_goal_metadata(&self, goal: Option<coco_session::GoalMetadata>) {
        self.runtime.persist_goal_metadata(goal).await;
    }

    pub async fn active_goal_snapshot(&self) -> Option<coco_types::ActiveGoal> {
        self.runtime.active_goal_snapshot().await
    }

    pub async fn restore_goal_from_history(
        &self,
        messages: &[Arc<coco_messages::Message>],
        trust_rejected: bool,
    ) -> Option<coco_types::ActiveGoal> {
        self.runtime
            .restore_goal_from_history(messages, trust_rejected)
            .await
    }

    pub async fn persist_local_transcript_messages(&self, messages: &[coco_messages::Message]) {
        self.runtime
            .persist_local_transcript_messages(messages)
            .await;
    }

    pub async fn append_messages_to_history(
        &self,
        messages: Vec<coco_messages::Message>,
    ) -> Vec<Arc<coco_messages::Message>> {
        self.runtime.append_messages_to_history(messages).await
    }

    pub async fn append_messages_to_history_and_emit(
        &self,
        messages: Vec<coco_messages::Message>,
        event_tx: Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        self.runtime
            .append_messages_to_history_and_emit(messages, event_tx)
            .await
    }

    pub async fn history_messages(&self) -> Vec<Arc<coco_messages::Message>> {
        self.runtime.history_messages().await
    }

    pub async fn truncate_history_at_user_message(
        &self,
        message_id: &str,
    ) -> Result<super::SessionHistoryTruncateResult, usize> {
        self.runtime
            .truncate_history_at_user_message(message_id)
            .await
    }

    pub async fn append_arc_messages_to_history_and_snapshot(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        self.runtime
            .append_arc_messages_to_history_and_snapshot(messages)
            .await
    }

    pub async fn replace_history_with_arc_messages(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) {
        self.runtime
            .replace_history_with_arc_messages(messages)
            .await;
    }

    pub async fn commit_engine_turn_history(&self, history: coco_messages::MessageHistory) {
        self.runtime.commit_engine_turn_history(history).await;
    }

    pub async fn commit_compacted_history(&self, history: coco_messages::MessageHistory) {
        self.runtime.commit_compacted_history(history).await;
    }

    pub async fn re_append_session_metadata(&self) {
        self.runtime.re_append_session_metadata().await;
    }

    pub async fn has_persisted_title(&self) -> bool {
        self.runtime.has_persisted_title().await
    }

    pub async fn persist_session_title(&self, name: String) -> anyhow::Result<()> {
        self.runtime.persist_session_title(name.clone()).await?;
        self.runtime.update_session_registry_name(&name);
        Ok(())
    }

    pub async fn title_generation_conversation_text(&self) -> String {
        self.runtime.title_generation_conversation_text().await
    }

    pub async fn list_persisted_session_summaries(
        &self,
    ) -> anyhow::Result<coco_types::SessionListResult> {
        self.runtime.list_persisted_session_summaries().await
    }

    pub async fn persist_session_mode(&self) {
        self.runtime.persist_session_mode().await;
    }

    pub fn reconcile_session_mode_on_resume(
        &self,
        stored_mode: Option<&str>,
    ) -> Option<&'static str> {
        self.runtime.reconcile_session_mode_on_resume(stored_mode)
    }

    pub async fn toggle_tag(&self, tag: String) -> anyhow::Result<(SessionId, bool)> {
        self.runtime.toggle_tag(tag).await
    }

    pub async fn rewind_files(
        &self,
        request: super::SessionFileRewindRequest,
    ) -> Result<super::SessionFileRewindResult, super::SessionFileRewindError> {
        self.runtime.rewind_files(request).await
    }

    pub async fn render_session_file_diff(
        &self,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        self.runtime.render_session_file_diff().await
    }

    /// Record a rewind restore point for `path` keyed by `message_id`,
    /// mirroring the engine's per-turn file-history capture. Exposed so
    /// rewind flows and conformance tests can seed a restore point without
    /// reaching into the runtime's `FileHistoryState` lock. No-op when file
    /// history is disabled for this session.
    pub async fn record_file_edit_for_rewind(
        &self,
        path: &std::path::Path,
        message_id: &str,
    ) -> anyhow::Result<()> {
        if let Some(file_history) = self.runtime.file_history() {
            file_history
                .write()
                .await
                .track_edit(
                    path,
                    message_id,
                    self.runtime.config_home(),
                    self.session_id.as_str(),
                )
                .await?;
        }
        Ok(())
    }

    /// Snapshot the last assistant text from history without exposing the
    /// underlying lock.
    pub async fn last_assistant_text(&self) -> Option<String> {
        self.runtime.history().lock().await.last_assistant_text()
    }

    pub async fn rewind_diff_stats(
        &self,
        message_id: &str,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        self.runtime.rewind_diff_stats(message_id).await
    }

    pub async fn rewind_diff_stats_between(
        &self,
        message_id: &str,
        next_message_id: Option<&str>,
    ) -> Result<Option<coco_context::DiffStats>, super::SessionFileDiffError> {
        self.runtime
            .rewind_diff_stats_between(message_id, next_message_id)
            .await
    }

    pub async fn render_turn_file_diff(
        &self,
        message_id: &str,
    ) -> Result<coco_context::RenderedDiff, super::SessionFileDiffError> {
        self.runtime.render_turn_file_diff(message_id).await
    }

    pub fn update_session_registry_name(&self, name: &str) {
        self.runtime.update_session_registry_name(name);
    }

    pub(crate) async fn seed_transcript_dedup<I>(&self, uuids: I)
    where
        I: IntoIterator<Item = uuid::Uuid>,
    {
        self.runtime.seed_transcript_dedup(uuids).await;
    }

    pub(crate) async fn seed_tool_result_replacement_state(
        &self,
        messages: &[coco_messages::Message],
        session_id: &SessionId,
        agent_id: Option<&str>,
    ) {
        self.runtime
            .seed_tool_result_replacement_state(messages, session_id, agent_id)
            .await;
    }

    pub async fn seed_todo_list_snapshot(&self, key: String, items: Vec<coco_types::TodoRecord>) {
        self.runtime.seed_todo_list_snapshot(key, items).await;
    }

    pub async fn clear_replacement_snapshot(&self) -> super::ClearReplacementSnapshot {
        self.runtime.clear_replacement_snapshot().await
    }

    pub(crate) async fn apply_clear_replacement_snapshot(
        &self,
        snapshot: super::ClearReplacementSnapshot,
    ) {
        self.runtime
            .apply_clear_replacement_snapshot(snapshot)
            .await;
    }

    pub async fn pre_clear_rewind_messages(&self) -> Option<Vec<Arc<coco_messages::Message>>> {
        self.runtime.pre_clear_rewind_messages().await
    }

    pub async fn restore_pre_clear_rewind_prefix(
        &self,
        message_id: &str,
    ) -> Option<(i32, i32, Vec<coco_messages::Message>)> {
        self.runtime
            .restore_pre_clear_rewind_prefix(message_id)
            .await
    }
}
