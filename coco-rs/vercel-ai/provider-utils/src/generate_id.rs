//! ID generation utilities.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique ID with a prefix.
///
/// Format: `{prefix}_{timestamp}_{counter}`
///
/// # Example
///
/// ```
/// use vercel_ai_provider_utils::generate_id;
///
/// let id = generate_id("msg");
/// assert!(id.starts_with("msg_"));
/// ```
pub fn generate_id(prefix: &str) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}_{timestamp}_{counter}")
}

/// Generate a unique ID without a prefix.
pub fn generate_id_simple() -> String {
    generate_id("id")
}

/// Generate a unique ID suitable for tool calls.
pub fn generate_tool_call_id() -> String {
    generate_id("call")
}

/// Generate a unique ID suitable for text segments.
pub fn generate_text_id() -> String {
    generate_id("txt")
}

/// Generate a unique ID suitable for reasoning segments.
pub fn generate_reasoning_id() -> String {
    generate_id("rsn")
}

/// Generate a random alphanumeric ID of the given length.
pub fn generate_random_id(length: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generate a UUID v4.
pub fn generate_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
#[path = "generate_id.test.rs"]
mod tests;
