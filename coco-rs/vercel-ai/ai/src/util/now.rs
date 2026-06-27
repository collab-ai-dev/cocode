//! Current timestamp utility.
//!
//! This module provides utilities for getting the current timestamp.

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Get the current Unix timestamp in seconds.
///
/// # Returns
///
/// The current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Get the current Unix timestamp in milliseconds.
///
/// # Returns
///
/// The current Unix timestamp in milliseconds.
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Get the current Unix timestamp in microseconds.
///
/// # Returns
///
/// The current Unix timestamp in microseconds.
pub fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Get the current Unix timestamp in nanoseconds.
///
/// # Returns
///
/// The current Unix timestamp in nanoseconds.
pub fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Get the current ISO 8601 timestamp.
///
/// # Returns
///
/// The current timestamp in ISO 8601 format.
pub fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Get the current timestamp as a chrono DateTime.
///
/// # Returns
///
/// The current timestamp.
pub fn now_datetime() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// Get the elapsed time since a timestamp.
///
/// # Arguments
///
/// * `start_secs` - The start timestamp in seconds.
///
/// # Returns
///
/// The elapsed time in seconds.
pub fn elapsed_secs(start_secs: u64) -> u64 {
    now_secs().saturating_sub(start_secs)
}

/// Get the elapsed time since a timestamp in milliseconds.
///
/// # Arguments
///
/// * `start_millis` - The start timestamp in milliseconds.
///
/// # Returns
///
/// The elapsed time in milliseconds.
pub fn elapsed_millis(start_millis: u64) -> u64 {
    now_millis().saturating_sub(start_millis)
}
