use coco_query::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

pub(super) async fn refresh_plugin_dialog_payload(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let payload = coco_agent_host::plugin_dialog::build_plugin_dialog_payload(session).await;
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::OpenPluginDialog { payload }))
        .await;
}
