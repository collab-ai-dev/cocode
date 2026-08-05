use super::session_errors::LifecycleError;

pub(crate) enum SessionOperationError {
    InvalidRequest {
        message: String,
        data: Option<serde_json::Value>,
    },
    InvalidParams {
        message: String,
        data: Option<serde_json::Value>,
    },
    Internal {
        message: String,
        data: Option<serde_json::Value>,
    },
    Lifecycle(LifecycleError),
}

pub(crate) enum SessionOperationErrorParts {
    InvalidRequest {
        message: String,
        data: Option<serde_json::Value>,
    },
    InvalidParams {
        message: String,
        data: Option<serde_json::Value>,
    },
    Internal {
        message: String,
        data: Option<serde_json::Value>,
    },
    Lifecycle(LifecycleError),
}

impl SessionOperationError {
    pub(crate) fn invalid_request(
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self::InvalidRequest {
            message: message.into(),
            data,
        }
    }

    pub(crate) fn invalid_params(
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self::InvalidParams {
            message: message.into(),
            data,
        }
    }

    pub(crate) fn internal(message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self::Internal {
            message: message.into(),
            data,
        }
    }

    pub(crate) fn into_parts(self) -> SessionOperationErrorParts {
        match self {
            Self::InvalidRequest { message, data } => {
                SessionOperationErrorParts::InvalidRequest { message, data }
            }
            Self::InvalidParams { message, data } => {
                SessionOperationErrorParts::InvalidParams { message, data }
            }
            Self::Internal { message, data } => {
                SessionOperationErrorParts::Internal { message, data }
            }
            Self::Lifecycle(error) => SessionOperationErrorParts::Lifecycle(error),
        }
    }

    pub(crate) fn into_registry_error(self) -> coco_app_server::RegistryError {
        match self.into_parts() {
            SessionOperationErrorParts::InvalidRequest { message, data }
            | SessionOperationErrorParts::InvalidParams { message, data } => {
                coco_app_server::RegistryError::load_rejected(message, data)
            }
            SessionOperationErrorParts::Internal { message, .. } => {
                coco_app_server::RegistryError::load_failed(message)
            }
            SessionOperationErrorParts::Lifecycle(error) => {
                let dispatch = error.into_dispatch_error();
                if dispatch.code == coco_types::error_codes::INTERNAL_ERROR {
                    coco_app_server::RegistryError::load_failed(dispatch.message)
                } else {
                    coco_app_server::RegistryError::load_rejected(dispatch.message, dispatch.data)
                }
            }
        }
    }
}

impl From<LifecycleError> for SessionOperationError {
    fn from(error: LifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}
