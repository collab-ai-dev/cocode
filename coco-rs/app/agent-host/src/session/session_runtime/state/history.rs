use super::*;

impl SessionRuntime {
    /// Shared multi-turn transcript for this runtime.
    pub fn history(&self) -> &Arc<Mutex<coco_messages::MessageHistory>> {
        self.history_resources.history()
    }
    pub async fn append_messages_to_history(
        &self,
        messages: Vec<coco_messages::Message>,
    ) -> Vec<Arc<coco_messages::Message>> {
        let mut history = self.history_resources.history().lock().await;
        messages
            .into_iter()
            .map(|message| {
                let message = Arc::new(message);
                history.push_arc(message.clone());
                message
            })
            .collect()
    }
    pub async fn append_messages_to_history_and_emit(
        &self,
        messages: Vec<coco_messages::Message>,
        event_tx: Option<mpsc::Sender<coco_types::CoreEvent>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        let mut history = self.history_resources.history().lock().await;
        for message in messages {
            coco_query::history_sync::history_push_and_emit(&mut history, message, &event_tx).await;
        }
        history.to_vec()
    }
    pub async fn history_messages(&self) -> Vec<Arc<coco_messages::Message>> {
        self.history_resources.history().lock().await.to_vec()
    }
    pub async fn truncate_history_at_user_message(
        &self,
        message_id: &str,
    ) -> Result<super::SessionHistoryTruncateResult, usize> {
        let mut history = self.history_resources.history().lock().await;
        let Some(idx) = history
            .as_slice()
            .iter()
            .position(|message| match message.as_ref() {
                Message::User(user) => user.uuid.to_string() == message_id,
                _ => false,
            })
        else {
            return Err(history.len());
        };
        let pre_count = history.len();
        history.truncate(idx);
        Ok(super::SessionHistoryTruncateResult {
            keep_count: idx,
            pre_count,
            removed: pre_count.saturating_sub(idx),
        })
    }
    pub async fn append_arc_messages_to_history_and_snapshot(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) -> Vec<Arc<coco_messages::Message>> {
        let mut history = self.history_resources.history().lock().await;
        for message in messages {
            history.push_arc(message);
        }
        history.to_vec()
    }
    async fn replace_history(&self, history: coco_messages::MessageHistory) {
        *self.history_resources.history().lock().await = history;
    }
    pub async fn replace_history_with_arc_messages(
        &self,
        messages: Vec<Arc<coco_messages::Message>>,
    ) {
        let mut history = coco_messages::MessageHistory::new();
        for message in messages {
            history.push_arc(message);
        }
        self.replace_history(history).await;
    }
    pub async fn commit_engine_turn_history(&self, history: coco_messages::MessageHistory) {
        self.replace_history(history).await;
    }
    pub async fn commit_compacted_history(&self, history: coco_messages::MessageHistory) {
        self.replace_history(history).await;
    }
}
