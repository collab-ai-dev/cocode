use std::sync::Arc;

use coco_messages::Message;

use super::SessionRuntime;

#[derive(Clone)]
pub struct ClearReplacementSnapshot {
    pub permissions: coco_types::LiveToolPermissionState,
    pub rewind_messages: Option<Vec<Arc<Message>>>,
}

impl SessionRuntime {
    pub async fn clear_replacement_snapshot(&self) -> ClearReplacementSnapshot {
        let permissions = self
            .engine_state_resources
            .app_state()
            .read()
            .await
            .permissions
            .clone();
        let rewind_messages = self.prepare_for_clear_replacement().await;
        ClearReplacementSnapshot {
            permissions,
            rewind_messages,
        }
    }

    pub(crate) async fn apply_clear_replacement_snapshot(
        &self,
        snapshot: ClearReplacementSnapshot,
    ) {
        {
            let mut app_state = self.engine_state_resources.app_state().write().await;
            app_state.permissions = snapshot.permissions;
        }
        self.seed_pre_clear_rewind_messages(snapshot.rewind_messages)
            .await;
    }

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

    pub(crate) async fn seed_pre_clear_rewind_messages(&self, messages: Option<Vec<Arc<Message>>>) {
        *self
            .engine_state_resources
            .clear_rewind_messages()
            .lock()
            .await = messages;
    }
}
