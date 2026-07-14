use coco_app_server::JsonRpcDispatchError;

use super::{AppServerHostState, RuntimeReplacementContext};

pub(crate) async fn require_runtime_replacement(
    state: &AppServerHostState,
    method: &'static str,
    include_stable_kind: bool,
) -> Result<RuntimeReplacementContext, JsonRpcDispatchError> {
    state
        .runtime_replacement_snapshot()
        .await
        .ok_or_else(|| runtime_factory_required_error(method, include_stable_kind))
}

fn runtime_factory_required_error(
    method: &'static str,
    include_stable_kind: bool,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: format!("{method} requires a runtime factory"),
        data: include_stable_kind
            .then(|| serde_json::json!({ "kind": "runtime_factory_required" })),
    }
}
