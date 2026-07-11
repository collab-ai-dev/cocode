use std::collections::HashMap;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use coco_app_server_transport::JsonRpcFrame;
use coco_types::RequestId;
use tokio::sync::Mutex;
use tokio::sync::oneshot;
use tracing::debug;
use tracing::warn;

/// Server-initiated JSON-RPC requests awaiting client replies.
pub(super) struct ServerRequestState {
    pending: Mutex<HashMap<RequestId, oneshot::Sender<JsonRpcFrame>>>,
    next_id: AtomicI64,
}

impl Default for ServerRequestState {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            // Start at -1 and decrement. Keeps us out of the typical
            // client-issued integer range and makes outbound IDs visually
            // distinctive in logs.
            next_id: AtomicI64::new(-1),
        }
    }
}

impl ServerRequestState {
    pub(super) async fn register_request(
        &self,
    ) -> (
        RequestId,
        oneshot::Receiver<JsonRpcFrame>,
        PendingServerRequestGuard<'_>,
    ) {
        let raw = self.next_id.fetch_sub(1, Ordering::SeqCst);
        let request_id = RequestId::Integer(raw);
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(request_id.clone(), tx);
        }
        let guard = PendingServerRequestGuard {
            map: &self.pending,
            request_id: request_id.clone(),
            active: true,
        };
        (request_id, rx, guard)
    }

    pub(super) async fn resolve_frame(&self, frame: JsonRpcFrame) -> bool {
        let request_id = match &frame {
            JsonRpcFrame::Success(success) => {
                match crate::sdk_server::transport::request_id_from_json_rpc_id(success.id.clone())
                {
                    Ok(id) => id,
                    Err(_) => return false,
                }
            }
            JsonRpcFrame::Error(error) => {
                match crate::sdk_server::transport::request_id_from_json_rpc_id(error.id.clone()) {
                    Ok(id) => id,
                    Err(_) => return false,
                }
            }
            JsonRpcFrame::Request(_) | JsonRpcFrame::Notification(_) => return false,
        };
        let mut map = self.pending.lock().await;
        let Some(sender) = map.remove(&request_id) else {
            debug!(
                request_id = %request_id.as_display(),
                "resolve_server_request_frame: no pending match"
            );
            return false;
        };
        if sender.send(frame).is_err() {
            warn!(
                request_id = %request_id.as_display(),
                "resolve_server_request_frame: receiver dropped before reply arrived"
            );
        }
        true
    }

    #[cfg(test)]
    pub(super) async fn len(&self) -> usize {
        self.pending.lock().await.len()
    }

    #[cfg(test)]
    pub(super) async fn is_empty(&self) -> bool {
        self.pending.lock().await.is_empty()
    }

    pub(super) fn next_id_for_debug(&self) -> i64 {
        self.next_id.load(Ordering::Relaxed)
    }
}

/// RAII cleanup for a pending `send_server_request` entry.
///
/// The guard uses `try_lock` in its sync `Drop` impl. If the mutex is
/// contended at drop time, the entry leaks; that window is bounded to other
/// short pending-map mutations and is reclaimed when the state is dropped.
pub(super) struct PendingServerRequestGuard<'a> {
    map: &'a Mutex<HashMap<RequestId, oneshot::Sender<JsonRpcFrame>>>,
    request_id: RequestId,
    active: bool,
}

impl PendingServerRequestGuard<'_> {
    pub(super) fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for PendingServerRequestGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut map) = self.map.try_lock() {
            map.remove(&self.request_id);
        }
    }
}
