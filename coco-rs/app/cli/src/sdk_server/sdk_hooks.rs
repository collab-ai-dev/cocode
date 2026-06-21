//! SDK hook callback bridge.
//!
//! Wires the agent-side hook orchestration to the SDK client over the
//! control-protocol transport. The bridge:
//!
//! 1. Installs a runtime callback on `HookRegistry` that, when invoked,
//!    sends a `hook/callback` server request and awaits the reply.
//! 2. Translates the SDK's typed [`coco_types::SdkHookOutput`] reply
//!    into a JSON `Value` for the existing hook-orchestration parser.
//!    There is **no behaviour collapse** — the SDK speaks the same
//!    canonical shape (`hookSpecificOutput` + flat sync fields) the
//!    orchestrator already understands.
//!
//! Concurrency note: every hook invocation gets a fresh JSON-RPC
//! `request_id` (issued by `send_server_request`). Two parallel
//! invocations of the same `callback_id` cannot consume each other's
//! responses.

use std::sync::Arc;

use tracing::warn;

use crate::sdk_server::handlers::SdkServerState;

pub fn install_runtime_callback(
    state: Arc<SdkServerState>,
    runtime: &Arc<crate::session_runtime::SessionRuntime>,
) {
    let callback: coco_hooks::SdkHookCallback = Arc::new(move |request| {
        let state = state.clone();
        Box::pin(async move { route_hook_callback(state, request).await })
    });
    runtime.hook_registry().set_sdk_hook_callback(callback);
}

pub fn register_initialize_hooks(
    runtime: &Arc<crate::session_runtime::SessionRuntime>,
    hooks: &std::collections::HashMap<
        coco_types::HookEventType,
        Vec<coco_types::HookCallbackMatcher>,
    >,
) -> usize {
    let registry = runtime.hook_registry();
    let mut count = 0;
    for (event, matchers) in hooks {
        for matcher in matchers {
            for callback_id in &matcher.hook_callback_ids {
                let timeout_ms = matcher.timeout.map(|seconds| seconds * 1000);
                let hook = coco_hooks::HookDefinition {
                    event: *event,
                    matcher: matcher.matcher.clone(),
                    handler: coco_hooks::HookHandler::SdkCallback {
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
                };
                if registry.register_deduped(hook) {
                    count += 1;
                }
            }
        }
    }
    count
}

async fn route_hook_callback(
    state: Arc<SdkServerState>,
    request: coco_hooks::SdkHookCallbackRequest,
) -> coco_hooks::Result<coco_types::SdkHookOutput> {
    let transport = {
        let guard = state.transport.read().await;
        guard.as_ref().cloned()
    }
    .ok_or_else(|| coco_hooks::HooksError::generic("SDK hook bridge transport not initialized"))?;

    let params = coco_types::ServerHookCallbackParams {
        callback_id: request.callback_id,
        event_type: request.event,
        input: request.input,
        tool_use_id: request.tool_use_id,
    };
    let params_json = serde_json::to_value(params).map_err(|e| {
        coco_hooks::HooksError::generic(format!("serialize hook/callback params: {e}"))
    })?;

    let reply = state
        .send_server_request(&transport, "hook/callback", params_json)
        .await
        .map_err(|e| coco_hooks::HooksError::generic(format!("send hook/callback: {e}")))?;

    match reply {
        coco_types::JsonRpcMessage::Response(response) => {
            // Strict typed parse — bad payload fails here instead of
            // getting silently re-interpreted by the legacy
            // `parse_hook_output` permissive parser. The typed output
            // flows end-to-end: callback → orchestration spawn loop
            // → `HookExecutionResult::SdkOutput` → `apply_sdk_hook_output`.
            // No JSON `Value` round-trip on this path.
            let result: coco_types::HookCallbackResult = serde_json::from_value(response.result)
                .map_err(|e| {
                    coco_hooks::HooksError::generic(format!("parse hook/callback response: {e}"))
                })?;
            Ok(result.output)
        }
        coco_types::JsonRpcMessage::Error(error) => Err(coco_hooks::HooksError::generic(format!(
            "SDK client returned hook/callback error: {} ({})",
            error.message, error.code
        ))),
        other => {
            warn!(?other, "unexpected hook/callback reply");
            Err(coco_hooks::HooksError::generic(
                "unexpected hook/callback reply",
            ))
        }
    }
}

#[cfg(test)]
#[path = "sdk_hooks.test.rs"]
mod tests;
