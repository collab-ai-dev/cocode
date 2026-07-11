use std::sync::Arc;

use coco_types::{ConnectionProfile, InitializeParams};
use tokio::sync::mpsc;

use super::*;

#[tokio::test]
async fn unscoped_handler_context_cannot_resolve_session_workspace() {
    let (notif_tx, _notif_rx) = mpsc::channel(1);
    let context = HandlerContext {
        notif_tx,
        state: Arc::new(SdkServerState::default()),
        connection_profile: Arc::new(
            ConnectionProfile::try_from(InitializeParams::default()).unwrap(),
        ),
        app_server: None,
        target_session_id: None,
        session: None,
    };
    assert!(context.resolve_runtime().await.is_none());
    assert!(context.workspace_cwd().await.is_err());
}
