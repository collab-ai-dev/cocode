//! Delay utilities for retry logic.

use std::time::Duration;

/// Delay for a specified duration.
pub async fn delay(duration: Duration) {
    tokio::time::sleep(duration).await;
}

/// Parse a Retry-After header value.
///
/// Can be either a number of seconds or an HTTP date.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    // Try parsing as seconds
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    // Try parsing as HTTP date
    // For simplicity, we only handle seconds here
    None
}

/// Calculate exponential backoff delay.
pub fn exponential_backoff(attempt: u32, base_delay: Duration, max_delay: Duration) -> Duration {
    let multiplier = 2u64.saturating_pow(attempt);
    let delay = base_delay.saturating_mul(multiplier.try_into().unwrap_or(u32::MAX));
    delay.min(max_delay)
}

/// Calculate jittered backoff delay.
pub fn jittered_backoff(attempt: u32, base_delay: Duration, max_delay: Duration) -> Duration {
    let base = exponential_backoff(attempt, base_delay, max_delay);
    // Add up to 50% jitter
    let jitter = base / 2;
    let jitter_amount = (jitter.as_millis() as f64 * rand::random::<f64>()) as u64;
    base + Duration::from_millis(jitter_amount)
}

#[cfg(test)]
#[path = "delay.test.rs"]
mod tests;
