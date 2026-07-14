use coco_app_server::JsonRpcDispatchError;
use coco_types::ClientRequest;

use super::session_operation_error::{SessionOperationError, SessionOperationErrorParts};
use super::{HandlerContext, HandlerResult};

pub(crate) fn encode_app_server_result<T: serde::Serialize>(
    result: T,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    serde_json::to_value(result).map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("AppServer result encode failed: {error}"),
        data: None,
    })
}

pub(crate) fn session_operation_error(error: SessionOperationError) -> JsonRpcDispatchError {
    match error.into_parts() {
        SessionOperationErrorParts::InvalidRequest { message, data } => JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message,
            data,
        },
        SessionOperationErrorParts::InvalidParams { message, data } => JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_PARAMS,
            message,
            data,
        },
        SessionOperationErrorParts::Internal { message, data } => JsonRpcDispatchError {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message,
            data,
        },
        SessionOperationErrorParts::Lifecycle(error) => error.into_dispatch_error(),
    }
}

pub async fn dispatch_app_server_client_request(
    request: ClientRequest,
    ctx: HandlerContext,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    match super::dispatch_client_request(request, ctx).await {
        HandlerResult::Ok(result) => Ok(result),
        HandlerResult::Err {
            code,
            message,
            data,
        } => Err(JsonRpcDispatchError {
            code,
            message,
            data,
        }),
        HandlerResult::NotImplemented(method) => {
            Err(JsonRpcDispatchError::method_not_found(method))
        }
    }
}
