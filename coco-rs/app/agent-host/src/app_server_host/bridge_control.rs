//! Production [`coco_bridge::ControlRequestHandler`] impl backed by
//! AppServer host state.
//!
//! The bridge crate defines the transport + trait; policy lives here
//! because `app/agent-host` is the layer that owns AppServer host state +
//! depends on `coco_permissions` / `coco_types`. Wiring looks like:
//!
//! ```ignore
//! let handler = Arc::new (AppServerBridgeControlHandler::new (state.clone()));
//! while let Some (msg) = incoming.recv().await {
//!     if let ReplInMessage::ControlRequest { request_id, request } = msg {
//!         let out = coco_bridge::dispatch_control(&*handler, request_id, request).await;
//!         bridge.send (out).await?;
//!     }
//! }
//! ```
//!
//! Security contract: every bypass-origin site
//! (TUI `UserCommand::SetPermissionMode`,
//! AppServer host `set_permission_mode`, and this bridge handler)
//! enforces the same rule — reject `BypassPermissions` when the
//! session's startup capability gate is off.

use std::sync::Arc;

use coco_bridge::{ControlError, ControlRequest, ControlRequestHandler};
use coco_types::ClientRequest;

use crate::app_server_host::{
    AppServerHostState, HandlerContext, HandlerResult, OutboundMessage, SessionRequestContext,
    dispatch_client_request,
};
use crate::session_controls;

/// Production handler for REPL-bridge control requests. Holds an
/// `Arc<AppServerHostState>` so it can read the bypass capability and mutate
/// the active session's permission mode through the same session capability
/// used by AppServer/TUI control paths.
pub struct AppServerBridgeControlHandler {
    state: Arc<AppServerHostState>,
    session: std::sync::RwLock<
        Option<(
            coco_types::InteractiveTarget,
            crate::session_runtime::SessionHandle,
        )>,
    >,
}

impl AppServerBridgeControlHandler {
    pub fn new(state: Arc<AppServerHostState>) -> Self {
        Self {
            state,
            session: std::sync::RwLock::new(None),
        }
    }

    pub fn bind_session(
        &self,
        target: coco_types::InteractiveTarget,
        session: crate::session_runtime::SessionHandle,
    ) {
        *self
            .session
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((target, session));
    }

    fn selected_session(
        &self,
    ) -> Option<(
        coco_types::InteractiveTarget,
        crate::session_runtime::SessionHandle,
    )> {
        self.session
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn set_permission_mode(
        &self,
        mode: coco_types::PermissionMode,
    ) -> Result<serde_json::Value, ControlError> {
        // Same guard as the shared AppServer handler + TUI runner — keep all three
        // bypass origins enforcing identical rules.
        if mode == coco_types::PermissionMode::BypassPermissions
            && !self.state.bypass_permissions_available()
        {
            return Err(ControlError::new(
                coco_types::error_codes::PERMISSION_DENIED,
                "Cannot set permission mode to bypassPermissions because \
                 the session was not launched with \
                 --dangerously-skip-permissions (or \
                 --allow-dangerously-skip-permissions).",
            ));
        }

        let Some((_target, runtime)) = self.selected_session() else {
            return Err(ControlError::new(
                coco_types::error_codes::INVALID_REQUEST,
                "no active session",
            ));
        };
        session_controls::set_permission_mode(Some(runtime), mode)
            .await
            .map_err(|error| {
                ControlError::new(coco_types::error_codes::INVALID_REQUEST, error.to_string())
            })?;

        Ok(serde_json::Value::Null)
    }

    async fn dispatch_control_request(
        &self,
        request: ClientRequest,
    ) -> Result<serde_json::Value, ControlError> {
        // Bridge-origin requests reuse the shared AppServer handler semantics. The local
        // channel is intentionally best-effort: routed bridge requests covered
        // here are single-shot controls/reads, while permission-mode changes
        // keep their explicit state-outbound path above.
        let (notif_tx, _notif_rx) = tokio::sync::mpsc::channel::<OutboundMessage>(16);
        let selected = self.selected_session();
        let target_session_id = selected
            .as_ref()
            .map(|(target, _)| target.session_id.clone());
        let session = selected.map(|(target, runtime)| SessionRequestContext {
            session_id: target.session_id,
            runtime,
        });
        let connection_profile =
            coco_types::ConnectionProfile::try_from(coco_types::InitializeParams::default())
                .map_err(|error| {
                    ControlError::new(
                        coco_types::error_codes::INTERNAL_ERROR,
                        format!("invalid built-in bridge connection profile: {error}"),
                    )
                })?;
        let ctx = HandlerContext {
            notif_tx,
            state: self.state.clone(),
            connection_profile: Arc::new(connection_profile),
            app_server: None,
            target_session_id,
            session,
        };
        match dispatch_client_request(request, ctx).await {
            HandlerResult::Ok(value) => Ok(value),
            HandlerResult::Err {
                code,
                message,
                data: _,
            } => Err(ControlError::new(code, message)),
            HandlerResult::NotImplemented(message) => Err(ControlError::new(
                coco_types::error_codes::METHOD_NOT_FOUND,
                message,
            )),
        }
    }
}

#[async_trait::async_trait]
impl ControlRequestHandler for AppServerBridgeControlHandler {
    async fn handle(&self, request: ControlRequest) -> Result<serde_json::Value, ControlError> {
        let interactive_target = || {
            self.selected_session()
                .map(|(target, _)| target)
                .ok_or_else(|| {
                    ControlError::new(
                        coco_types::error_codes::INVALID_REQUEST,
                        "bridge control requires an explicitly bound interactive session",
                    )
                })
        };
        match request {
            ControlRequest::Initialize { system_prompt } => {
                self.dispatch_control_request(ClientRequest::Initialize(
                    coco_types::InitializeParams {
                        system_prompt,
                        ..Default::default()
                    },
                ))
                .await
            }
            ControlRequest::Interrupt => {
                self.dispatch_control_request(ClientRequest::TurnInterrupt(interactive_target()?))
                    .await
            }
            ControlRequest::SetModel { model } => {
                self.dispatch_control_request(ClientRequest::SetModel(coco_types::SetModelParams {
                    target: interactive_target()?,
                    model,
                }))
                .await
            }
            ControlRequest::SetPermissionMode { mode } => self.set_permission_mode(mode).await,
            ControlRequest::McpStatus => {
                let target = interactive_target()?;
                self.dispatch_control_request(ClientRequest::McpStatus(coco_types::SessionTarget {
                    session_id: target.session_id,
                }))
                .await
            }
            ControlRequest::GetContextUsage => {
                let target = interactive_target()?;
                self.dispatch_control_request(ClientRequest::ContextUsage(
                    coco_types::SessionTarget {
                        session_id: target.session_id,
                    },
                ))
                .await
            }
            ControlRequest::RewindFiles {
                user_message_id,
                dry_run,
            } => {
                self.dispatch_control_request(ClientRequest::RewindFiles(
                    coco_types::RewindFilesParams {
                        target: interactive_target()?,
                        user_message_id,
                        dry_run,
                    },
                ))
                .await
            }
        }
    }
}
