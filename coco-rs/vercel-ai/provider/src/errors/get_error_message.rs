//! Get error message utility.
//!
//! Extracts a human-readable error message from various error types.

/// Get a human-readable error message from any error.
///
/// This function attempts to extract a meaningful error message from
/// the given error, falling back to the Debug representation if needed.
pub fn get_error_message(error: &dyn std::error::Error) -> String {
    error.to_string()
}

#[cfg(test)]
#[path = "get_error_message.test.rs"]
mod tests;
