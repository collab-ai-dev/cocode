use serde::Deserialize;
use serde::Serialize;

/// Result of input validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result")]
pub enum ValidationResult {
    #[serde(rename = "valid")]
    Valid,
    #[serde(rename = "invalid")]
    Invalid {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
}

impl ValidationResult {
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
            error_code: None,
        }
    }

    pub fn invalid_with_code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
            error_code: Some(code.into()),
        }
    }
}
