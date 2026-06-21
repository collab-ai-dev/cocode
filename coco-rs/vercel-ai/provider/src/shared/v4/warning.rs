//! Warning types (V4).
//!
//! Warnings provide information about non-fatal issues that occurred
//! during request processing.

use serde::Deserialize;
use serde::Serialize;

/// A warning returned by the provider or model.
///
/// Matches the `SharedV4Warning` type from the Vercel AI SDK v4 spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Warning {
    /// A feature is not supported by the provider.
    Unsupported {
        /// The unsupported feature.
        feature: String,
        /// Additional details about the unsupported feature.
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// A compatibility issue was detected.
    Compatibility {
        /// The feature with compatibility issues.
        feature: String,
        /// Additional details about the compatibility issue.
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// Other/unspecified warning.
    Other {
        /// The warning message.
        message: String,
    },
}

impl Warning {
    /// Create an unsupported feature warning.
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Self::Unsupported {
            feature: feature.into(),
            details: None,
        }
    }

    /// Create an unsupported feature warning with details.
    pub fn unsupported_with_details(
        feature: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self::Unsupported {
            feature: feature.into(),
            details: Some(details.into()),
        }
    }

    /// Create a compatibility warning.
    pub fn compatibility(feature: impl Into<String>) -> Self {
        Self::Compatibility {
            feature: feature.into(),
            details: None,
        }
    }

    /// Create a compatibility warning with details.
    pub fn compatibility_with_details(
        feature: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self::Compatibility {
            feature: feature.into(),
            details: Some(details.into()),
        }
    }

    /// Create an other warning.
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other {
            message: message.into(),
        }
    }
}

#[cfg(test)]
#[path = "warning.test.rs"]
mod tests;
