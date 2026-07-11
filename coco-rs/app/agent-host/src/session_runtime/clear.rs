use std::sync::Arc;

use coco_messages::Message;

use super::SessionRuntime;

impl SessionRuntime {
    pub async fn prepare_for_clear_replacement(&self) -> Option<Vec<Arc<Message>>> {
        let pre_clear_messages = self
            .history_resources
            .history()
            .lock()
            .await
            .as_slice()
            .to_vec();
        let rewind_messages = pre_clear_messages
            .iter()
            .any(|m| matches!(m.as_ref(), Message::User(_)))
            .then_some(pre_clear_messages);
        *self
            .engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await = rewind_messages.clone();
        rewind_messages
    }

    pub async fn seed_pre_clear_rewind_messages(&self, messages: Option<Vec<Arc<Message>>>) {
        *self
            .engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await = messages;
    }
}
