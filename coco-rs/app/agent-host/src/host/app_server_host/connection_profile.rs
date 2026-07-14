use std::sync::{Arc, OnceLock};

use coco_app_server::JsonRpcDispatchError;
use coco_types::{ClientRequest, ConnectionProfile};

pub(crate) type ConnectionProfileSlot = Arc<OnceLock<ConnectionProfile>>;

pub(crate) fn empty_connection_profile_slot() -> ConnectionProfileSlot {
    Arc::new(OnceLock::new())
}

pub(crate) fn local_connection_profile_slot() -> ConnectionProfileSlot {
    let slot = empty_connection_profile_slot();
    let profile = match ConnectionProfile::try_from(coco_types::InitializeParams::default()) {
        Ok(profile) => profile,
        Err(error) => panic!("invalid built-in local connection profile: {error}"),
    };
    let _ = slot.set(profile);
    slot
}

pub(crate) fn resolve_connection_profile_for_request(
    slot: &ConnectionProfileSlot,
    require_initialize: bool,
    request: &ClientRequest,
) -> Result<Arc<ConnectionProfile>, JsonRpcDispatchError> {
    if let ClientRequest::Initialize(params) = request {
        if slot.get().is_some() {
            return Err(already_initialized_error());
        }
        let profile =
            ConnectionProfile::try_from(params.clone()).map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_PARAMS,
                message: error.to_string(),
                data: None,
            })?;
        slot.set(profile).map_err(|_| already_initialized_error())?;
    }
    slot.get()
        .cloned()
        .map(Arc::new)
        .ok_or_else(|| not_initialized_error(require_initialize))
}

fn already_initialized_error() -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: "connection is already initialized".to_string(),
        data: Some(serde_json::json!({ "kind": "already_initialized" })),
    }
}

fn not_initialized_error(require_initialize: bool) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: if require_initialize {
            "connection is not initialized"
        } else {
            "local connection profile is unavailable"
        }
        .to_string(),
        data: Some(serde_json::json!({ "kind": "not_initialized" })),
    }
}
