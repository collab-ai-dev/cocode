use std::sync::Arc;

use coco_types::{ConnectionProfile, InitializeParams};
use tokio::sync::mpsc;

use super::*;
use crate::app_server_host::AppServerHostState;

#[tokio::test]
async fn unscoped_handler_context_cannot_resolve_runtime() {
    let (notif_tx, _notif_rx) = mpsc::channel(1);
    let context = HandlerContext {
        notif_tx,
        state: Arc::new(AppServerHostState::default()),
        connection_profile: Arc::new(
            ConnectionProfile::try_from(InitializeParams::default()).unwrap(),
        ),
        app_server: None,
        connection: None,
        target_session_id: None,
        session: None,
    };
    assert!(context.resolve_runtime().await.is_none());
}
