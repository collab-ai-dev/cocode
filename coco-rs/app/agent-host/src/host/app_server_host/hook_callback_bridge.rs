//! AppServer-routed hook callback bridge.
//!
//! Wires agent-side hook orchestration to the active interactive surface
//! through AppServer server requests. The bridge:
//!
//! 1. Installs a runtime callback on `HookRegistry` that, when invoked,
//!    sends a `hook/callback` server request and awaits the reply.
//! 2. Translates the typed [`coco_types::HookCallbackOutput`] reply into the
//!    canonical shape the orchestrator already understands.
//!
//! Concurrency note: every hook invocation gets a fresh JSON-RPC
//! `request_id` (issued by `send_server_request`). Two parallel
//! invocations of the same `callback_id` cannot consume each other's
//! responses.

use std::sync::Arc;

use tracing::warn;

use crate::{app_session::AppSessionHandle, session_runtime::SessionHandle};

pub fn install_runtime_callback(
    app_server: Arc<coco_app_server::AppServer<AppSessionHandle>>,
    session: &SessionHandle,
) {
    let callback_session = session.clone();
    let callback: coco_hooks::ClientHookCallback = Arc::new(move |request| {
        let app_server = Arc::clone(&app_server);
        let session = callback_session.clone();
        Box::pin(async move { route_hook_callback(app_server, session, request).await })
    });
    session.set_client_hook_callback(callback);
}

pub fn register_initialize_hooks(
    session: &SessionHandle,
    hooks: &std::collections::HashMap<
        coco_types::HookEventType,
        Vec<coco_types::HookCallbackMatcher>,
    >,
) -> usize {
    let mut definitions = Vec::new();
    for (event, matchers) in hooks {
        for matcher in matchers {
            for callback_id in &matcher.hook_callback_ids {
                let timeout_ms = matcher.timeout.map(|seconds| seconds * 1000);
                definitions.push(coco_hooks::HookDefinition {
                    event: *event,
                    matcher: matcher.matcher.clone(),
                    handler: coco_hooks::HookHandler::ClientCallback {
                        callback_id: callback_id.clone(),
                        timeout_ms,
                    },
                    priority: 0,
                    scope: coco_types::HookScope::Session,
                    if_condition: None,
                    once: false,
                    is_async: false,
                    async_rewake: false,
                    status_message: None,
                    managed_by: None,
                });
            }
        }
    }
    session.register_hook_definitions(definitions)
}

async fn route_hook_callback(
    app_server: Arc<coco_app_server::AppServer<AppSessionHandle>>,
    session: SessionHandle,
    request: coco_hooks::ClientHookCallbackRequest,
) -> coco_hooks::Result<coco_types::HookCallbackOutput> {
    let params = coco_types::ServerHookCallbackParams {
        callback_id: request.callback_id,
        event_type: request.event,
        input: request.input,
        tool_use_id: request.tool_use_id,
    };
    let reply = app_server
        .route_server_request_with_reply(
            session.session_id().clone(),
            coco_app_server::SurfaceCapability::Interactive,
            None,
            coco_types::ServerRequest::HookCallback(params),
        )
        .map_err(|e| coco_hooks::HooksError::generic(format!("route hook/callback: {e:?}")))?
        .await
        .map_err(|_| coco_hooks::HooksError::generic("hook/callback reply channel closed"))?;

    match reply {
        coco_app_server::ServerRequestReply::HookCallback { result, .. } => {
            // Strict typed parse — bad payload fails here instead of
            // getting silently re-interpreted by the legacy
            // `parse_hook_output` permissive parser. The typed output
            // flows end-to-end: callback → orchestration spawn loop
            // → `HookExecutionResult::ClientOutput` → `apply_client_hook_output`.
            // No JSON `Value` round-trip on this path.
            let result: coco_types::HookCallbackResult =
                serde_json::from_value(result).map_err(|e| {
                    coco_hooks::HooksError::generic(format!("parse hook/callback response: {e}"))
                })?;
            Ok(result.output)
        }
        coco_app_server::ServerRequestReply::Error(error) => {
            Err(coco_hooks::HooksError::generic(format!(
                "remote client returned hook/callback error: {} ({})",
                error.message, error.code
            )))
        }
        other => {
            warn!(?other, "unexpected hook/callback reply");
            Err(coco_hooks::HooksError::generic(
                "unexpected hook/callback reply",
            ))
        }
    }
}

#[cfg(test)]
#[path = "hook_callback_bridge.test.rs"]
mod tests;
