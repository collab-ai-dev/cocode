//! Retry logic with exponential backoff.
//!
//! This module provides retry functionality for transient failures.

use std::future::Future;
use std::time::Duration;

use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Initial delay in milliseconds before the first retry.
    pub initial_delay_ms: u64,
    /// Maximum delay in milliseconds between retries.
    pub max_delay_ms: u64,
    /// Multiplier for exponential backoff.
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// Create a new retry configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the initial delay in milliseconds.
    pub fn with_initial_delay_ms(mut self, initial_delay_ms: u64) -> Self {
        self.initial_delay_ms = initial_delay_ms;
        self
    }

    /// Set the maximum delay in milliseconds.
    pub fn with_max_delay_ms(mut self, max_delay_ms: u64) -> Self {
        self.max_delay_ms = max_delay_ms;
        self
    }

    /// Set the backoff multiplier.
    pub fn with_multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }

    /// Calculate the delay for a given attempt.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay_ms = if attempt == 0 {
            self.initial_delay_ms
        } else {
            let calculated = (self.initial_delay_ms as f64) * self.multiplier.powi(attempt as i32);
            calculated.min(self.max_delay_ms as f64) as u64
        };
        Duration::from_millis(delay_ms)
    }
}

/// Trait for determining if an error is retryable.
pub trait RetryableError {
    /// Returns true if the error is transient and the operation should be retried.
    fn is_retryable(&self) -> bool;
}

/// Execute an async operation with retry logic.
///
/// This function will retry the operation up to `max_retries` times with
/// exponential backoff between attempts.
///
/// # Arguments
///
/// * `config` - The retry configuration.
/// * `abort_signal` - Optional cancellation token.
/// * `f` - The async operation to execute.
///
/// # Returns
///
/// The result of the operation, or the last error if all retries failed.
pub async fn with_retry<T, E, F, Fut>(
    config: RetryConfig,
    abort_signal: Option<CancellationToken>,
    mut f: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: RetryableError + std::fmt::Debug,
{
    let mut attempt = 0u32;

    loop {
        match f().await {
            Ok(result) => return Ok(result),
            Err(error) => {
                // Check for cancellation after error
                if let Some(ref signal) = abort_signal
                    && signal.is_cancelled()
                {
                    return Err(error);
                }

                // Check if error is retryable
                if !error.is_retryable() || attempt >= config.max_retries {
                    return Err(error);
                }

                // Wait before retrying
                let delay = config.delay_for_attempt(attempt);
                sleep(delay).await;

                attempt += 1;
            }
        }
    }
}

// Implement RetryableError for AIError from the error module
impl crate::error::AIError {
    /// Check if this error is retryable.
    pub fn is_retryable_error(&self) -> bool {
        match self {
            Self::ProviderError(e) => {
                let error_str = e.to_string().to_lowercase();
                error_str.contains("timeout")
                    || error_str.contains("rate limit")
                    || error_str.contains("overloaded")
                    || error_str.contains("503")
                    || error_str.contains("429")
                    || error_str.contains("500")
            }
            _ => false,
        }
    }
}

impl RetryableError for crate::error::AIError {
    fn is_retryable(&self) -> bool {
        self.is_retryable_error()
    }
}

#[cfg(test)]
#[path = "retry.test.rs"]
mod tests;
