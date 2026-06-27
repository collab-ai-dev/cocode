//! Load setting error type.

use std::fmt;
use thiserror::Error;

/// Error thrown when a setting cannot be loaded.
#[derive(Debug, Error)]
pub struct LoadSettingError {
    /// The error message.
    pub message: String,
}

impl fmt::Display for LoadSettingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Load setting error: {}", self.message)
    }
}

impl LoadSettingError {
    /// Create a new load setting error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[cfg(test)]
#[path = "load_setting_error.test.rs"]
mod tests;
