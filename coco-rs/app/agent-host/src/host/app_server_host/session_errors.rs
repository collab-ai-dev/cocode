use coco_app_server::JsonRpcDispatchError;

#[derive(Debug)]
pub(crate) enum LifecycleError {
    InvalidRequest {
        message: String,
        data: Option<serde_json::Value>,
    },
    InvalidParams {
        message: String,
        data: Option<serde_json::Value>,
    },
    PermissionDenied {
        message: String,
        data: Option<serde_json::Value>,
    },
    Internal {
        message: String,
        data: Option<serde_json::Value>,
    },
}

impl LifecycleError {
    pub(crate) fn into_dispatch_error(self) -> JsonRpcDispatchError {
        match self {
            Self::InvalidRequest { message, data } => JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_REQUEST,
                message,
                data,
            },
            Self::InvalidParams { message, data } => JsonRpcDispatchError {
                code: coco_types::error_codes::INVALID_PARAMS,
                message,
                data,
            },
            Self::PermissionDenied { message, data } => JsonRpcDispatchError {
                code: coco_types::error_codes::PERMISSION_DENIED,
                message,
                data,
            },
            Self::Internal { message, data } => JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message,
                data,
            },
        }
    }
}

pub(crate) fn local_lifecycle_error(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> JsonRpcDispatchError {
    local_lifecycle_error_parts(operation, error).into_dispatch_error()
}

pub(crate) fn local_lifecycle_error_parts(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> LifecycleError {
    LifecycleError::Internal {
        message: format!("local AppServer {operation} failed: {error}"),
        data: None,
    }
}

pub(crate) fn app_server_lifecycle_error(
    operation: &'static str,
    error: coco_app_server::AppServerError,
) -> JsonRpcDispatchError {
    app_server_lifecycle_error_parts(operation, error).into_dispatch_error()
}

pub(crate) fn app_server_lifecycle_error_parts(
    operation: &'static str,
    error: coco_app_server::AppServerError,
) -> LifecycleError {
    use coco_app_server::AppServerError;

    let message = format!("local AppServer {operation} failed: {error}");
    let (kind, data) = match &error {
        AppServerError::Registry { source, .. } => {
            return registry_lifecycle_error_parts(operation, source.clone());
        }
        AppServerError::Attach { source, .. } => {
            return attach_lifecycle_error_parts(operation, source.clone());
        }
        AppServerError::SessionNotAttached { session_id, .. } => (
            LifecycleErrorKind::InvalidParams,
            serde_json::json!({ "kind": "session_not_attached", "session_id": session_id }),
        ),
        AppServerError::SessionGrantMissing { session_id, .. } => (
            LifecycleErrorKind::PermissionDenied,
            serde_json::json!({ "kind": "session_grant_missing", "session_id": session_id }),
        ),
        AppServerError::SessionGrantReadOnly { session_id, .. } => (
            LifecycleErrorKind::PermissionDenied,
            serde_json::json!({ "kind": "session_grant_read_only", "session_id": session_id }),
        ),
        AppServerError::TargetSessionNotLive {
            session_id, state, ..
        } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "target_session_not_live", "session_id": session_id, "state": state }),
        ),
        AppServerError::ServerRequestNotFound { request_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "server_request_not_found", "request_id": request_id }),
        ),
        AppServerError::ServerRequestWrongSession {
            request_id,
            expected_session_id,
            actual_session_id,
            ..
        } => (
            LifecycleErrorKind::PermissionDenied,
            serde_json::json!({
                "kind": "server_request_wrong_session",
                "request_id": request_id,
                "expected_session_id": expected_session_id,
                "actual_session_id": actual_session_id,
            }),
        ),
        AppServerError::ServerRequestNotRecipient { request_id, .. } => (
            LifecycleErrorKind::PermissionDenied,
            serde_json::json!({
                "kind": "server_request_not_recipient",
                "request_id": request_id,
            }),
        ),
        AppServerError::ServerRequestWrongReplyKind {
            request_id,
            expected,
            actual,
            ..
        } => (
            LifecycleErrorKind::InvalidParams,
            serde_json::json!({
                "kind": "server_request_wrong_reply_kind",
                "request_id": request_id,
                "expected": format!("{expected:?}"),
                "actual": format!("{actual:?}"),
            }),
        ),
    };
    lifecycle_error(kind, message, Some(data))
}

pub(crate) fn attach_lifecycle_error_parts(
    operation: &'static str,
    error: coco_app_server::AttachError,
) -> LifecycleError {
    let message = format!("local AppServer {operation} failed: {error}");
    let (kind, data) = match &error {
        coco_app_server::AttachError::ConnectionNotRegistered { .. } => (
            LifecycleErrorKind::InvalidParams,
            serde_json::json!({ "kind": "connection_not_registered" }),
        ),
        coco_app_server::AttachError::ConnectionAttachmentLimit {
            max_attached_sessions_per_connection,
            ..
        } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({
                "kind": "connection_attachment_limit",
                "max_attached_sessions_per_connection": max_attached_sessions_per_connection,
            }),
        ),
        coco_app_server::AttachError::SessionConnectionLimit {
            session_id,
            max_connections_per_session,
            ..
        } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({
                "kind": "session_connection_limit",
                "session_id": session_id,
                "max_connections_per_session": max_connections_per_session,
            }),
        ),
        coco_app_server::AttachError::SessionClosing { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "target_session_not_live", "session_id": session_id, "state": "closing" }),
        ),
        coco_app_server::AttachError::SessionNotFound { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "target_session_not_live", "session_id": session_id, "state": "missing" }),
        ),
        coco_app_server::AttachError::ReplayQueueUnavailable { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "replay_queue_unavailable", "session_id": session_id }),
        ),
    };
    lifecycle_error(kind, message, Some(data))
}

pub(crate) fn registry_lifecycle_error_parts(
    operation: &'static str,
    error: coco_app_server::RegistryError,
) -> LifecycleError {
    use coco_app_server::RegistryError;

    let message = format!("local AppServer {operation} failed: {error}");
    let (kind, data) = match &error {
        RegistryError::NotFound { session_id, .. } => (
            LifecycleErrorKind::InvalidParams,
            serde_json::json!({ "kind": "session_not_found", "session_id": session_id }),
        ),
        RegistryError::ResourceExhausted { .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "session_capacity_exhausted" }),
        ),
        RegistryError::OldNotReady { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "source_session_not_live", "session_id": session_id }),
        ),
        RegistryError::NewSlotOccupied { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "destination_session_occupied", "session_id": session_id }),
        ),
        RegistryError::ChildExists { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "child_sidechat_exists", "session_id": session_id }),
        ),
        RegistryError::SlotConflict {
            session_id,
            expected,
            ..
        } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "session_slot_conflict", "session_id": session_id, "expected": expected }),
        ),
        RegistryError::LoadFailed { .. } | RegistryError::SignalDropped { .. } => (
            LifecycleErrorKind::Internal,
            serde_json::json!({ "kind": "session_operation_internal" }),
        ),
        RegistryError::CloseFailed { data, .. } => (
            LifecycleErrorKind::Internal,
            data.clone()
                .unwrap_or_else(|| serde_json::json!({ "kind": "session_close_failed" })),
        ),
        RegistryError::CloseAborted { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "session_close_aborted", "session_id": session_id }),
        ),
        RegistryError::DeleteInProgress { session_id, .. } => (
            LifecycleErrorKind::InvalidRequest,
            serde_json::json!({ "kind": "session_delete_in_progress", "session_id": session_id }),
        ),
    };
    lifecycle_error(kind, message, Some(data))
}

enum LifecycleErrorKind {
    InvalidRequest,
    InvalidParams,
    PermissionDenied,
    Internal,
}

fn lifecycle_error(
    kind: LifecycleErrorKind,
    message: String,
    data: Option<serde_json::Value>,
) -> LifecycleError {
    match kind {
        LifecycleErrorKind::InvalidRequest => LifecycleError::InvalidRequest { message, data },
        LifecycleErrorKind::InvalidParams => LifecycleError::InvalidParams { message, data },
        LifecycleErrorKind::PermissionDenied => LifecycleError::PermissionDenied { message, data },
        LifecycleErrorKind::Internal => LifecycleError::Internal { message, data },
    }
}
