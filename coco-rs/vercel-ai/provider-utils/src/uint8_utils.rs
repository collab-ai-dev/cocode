//! Utility functions for working with byte arrays.
//!
//! Provides functions for converting between base64 strings and byte arrays.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// Convert a base64 string to a byte array.
///
/// Handles both standard base64 and URL-safe base64 encoding.
///
/// # Arguments
///
/// * `base64_string` - A base64 encoded string (standard or URL-safe).
///
/// # Returns
///
/// A `Vec<u8>` containing the decoded bytes.
pub fn convert_base64_to_bytes(base64_string: &str) -> Vec<u8> {
    // Handle URL-safe base64 by converting to standard base64
    let standard_base64 = base64_string.replace('-', "+").replace('_', "/");

    STANDARD.decode(&standard_base64).unwrap_or_default()
}

/// Convert a byte array to a base64 string.
///
/// # Arguments
///
/// * `bytes` - A byte slice to encode.
///
/// # Returns
///
/// A base64 encoded string.
pub fn convert_bytes_to_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Convert a value to base64.
///
/// If the value is already a string, it is returned as-is.
/// If the value is bytes, it is encoded as base64.
///
/// # Arguments
///
/// * `value` - Either a string or bytes.
///
/// # Returns
///
/// A base64 encoded string (or the original string if already a string).
pub fn convert_to_base64(value: &[u8]) -> String {
    convert_bytes_to_base64(value)
}

#[cfg(test)]
#[path = "uint8_utils.test.rs"]
mod tests;
