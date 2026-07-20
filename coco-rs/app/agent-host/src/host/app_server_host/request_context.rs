use std::{path::PathBuf, sync::Arc};

use serde_json::Value;
use tokio::sync::mpsc;

use super::AppServerHostState;
use crate::app_server_host::OutboundMessage;

/// Per-request context passed to AppServer host handlers.
pub struct HandlerContext {
    /// Channel for forwarding CoreEvent notifications to the active adapter.
    pub notif_tx: mpsc::Sender<OutboundMessage>,

    /// Shared host state across requests.
    pub state: Arc<AppServerHostState>,

    /// Immutable initialize snapshot owned by this accepted connection.
    pub connection_profile: Arc<coco_types::ConnectionProfile>,

    pub app_server: Option<Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>>,

    pub connection: Option<coco_app_server::ConnectionKey>,

    /// Explicit protocol target, including persisted-session requests that do
    /// not require a live runtime.
    pub target_session_id: Option<coco_types::SessionId>,

    /// Validated live-session capability. Whenever present, the id and handle
    /// were resolved together from AppServer; they cannot describe different
    /// sessions.
    pub session: Option<SessionRequestContext>,
}

#[derive(Clone)]
pub struct SessionRequestContext {
    pub session_id: coco_types::SessionId,
    pub runtime: crate::session_runtime::SessionHandle,
}

impl HandlerContext {
    pub fn has_scoped_session(&self) -> bool {
        self.session.is_some()
    }

    pub async fn active_session_id(&self) -> Option<coco_types::SessionId> {
        self.session
            .as_ref()
            .map(|session| session.session_id.clone())
    }

    /// Resolve only the runtime selected and validated by AppServer routing.
    pub(crate) async fn resolve_runtime(&self) -> Option<crate::session_runtime::SessionHandle> {
        self.session.as_ref().map(|session| session.runtime.clone())
    }

    pub(crate) async fn workspace_cwd(&self) -> Result<PathBuf, HandlerResult> {
        if let Some(session) = &self.session {
            return Ok(session.runtime.original_cwd().clone());
        }
        Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "request requires an explicitly targeted session workspace".to_string(),
            data: None,
        })
    }
}

/// Result of dispatching a ClientRequest.
pub enum HandlerResult {
    /// Handler succeeded -- carries the response `result` payload.
    Ok(Value),
    /// Handler failed with a JSON-RPC error.
    Err {
        code: i32,
        message: String,
        data: Option<Value>,
    },
    /// Handler is not implemented. The dispatcher converts this to a
    /// `JsonRpcError` with `METHOD_NOT_FOUND`.
    NotImplemented(String),
}

impl HandlerResult {
    pub fn ok_empty() -> Self {
        Self::Ok(Value::Null)
    }

    pub fn ok<T: serde::Serialize>(value: T) -> Self {
        match serde_json::to_value(value) {
            Ok(v) => Self::Ok(v),
            Err(e) => Self::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("result serialization failed: {e}"),
                data: None,
            },
        }
    }
}

#[cfg(test)]
#[path = "request_context.test.rs"]
mod tests;
