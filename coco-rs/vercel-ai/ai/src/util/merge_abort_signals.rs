//! Merge abort signals for combined cancellation.
//!
//! This module provides utilities for merging multiple cancellation
//! tokens/signals into a single combined signal.

use tokio_util::sync::CancellationToken;

/// Merge multiple cancellation tokens into one.
///
/// The merged token will be cancelled when any of the input tokens
/// is cancelled.
///
/// # Arguments
///
/// * `tokens` - Slice of cancellation tokens to merge.
///
/// # Returns
///
/// A new `CancellationToken` that is cancelled when any input is cancelled.
pub fn merge_abort_signals(tokens: &[CancellationToken]) -> CancellationToken {
    let merged = CancellationToken::new();

    if tokens.is_empty() {
        return merged;
    }

    // Clone for each spawned task
    let merged_clone = merged.clone();

    for token in tokens {
        let merged = merged_clone.clone();
        let token = token.clone();

        tokio::spawn(async move {
            token.cancelled().await;
            merged.cancel();
        });
    }

    merged
}

/// Create a timeout cancellation token.
///
/// # Arguments
///
/// * `duration` - The timeout duration.
///
/// # Returns
///
/// A `CancellationToken` that will be cancelled after the duration.
pub fn create_timeout_token(duration: std::time::Duration) -> CancellationToken {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        token_clone.cancel();
    });

    token
}

/// Create a deadline cancellation token.
///
/// # Arguments
///
/// * `deadline` - The instant when the token should be cancelled.
///
/// # Returns
///
/// A `CancellationToken` that will be cancelled at the deadline.
pub fn create_deadline_token(deadline: std::time::Instant) -> CancellationToken {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    tokio::spawn(async move {
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        token_clone.cancel();
    });

    token
}

/// Merge abort signals with a timeout.
///
/// # Arguments
///
/// * `tokens` - Slice of cancellation tokens.
/// * `timeout` - Optional timeout duration.
///
/// # Returns
///
/// A merged token that includes the timeout.
pub fn merge_abort_signals_with_timeout(
    tokens: &[CancellationToken],
    timeout: Option<std::time::Duration>,
) -> CancellationToken {
    let mut all_tokens = tokens.to_vec();

    if let Some(duration) = timeout {
        all_tokens.push(create_timeout_token(duration));
    }

    merge_abort_signals(&all_tokens)
}

/// Cancellation helper for managing multiple signals.
pub struct CancellationManager {
    /// The primary cancellation token.
    primary: CancellationToken,
    /// Child tokens that will be cancelled when the primary is.
    children: Vec<CancellationToken>,
}

impl CancellationManager {
    /// Create a new cancellation manager.
    pub fn new() -> Self {
        Self {
            primary: CancellationToken::new(),
            children: Vec::new(),
        }
    }

    /// Create with an existing token.
    pub fn with_token(token: CancellationToken) -> Self {
        Self {
            primary: token,
            children: Vec::new(),
        }
    }

    /// Get the primary token.
    pub fn token(&self) -> CancellationToken {
        self.primary.clone()
    }

    /// Create a child token.
    ///
    /// The child will be cancelled when the primary is cancelled.
    pub fn child_token(&mut self) -> CancellationToken {
        let child = self.primary.child_token();
        self.children.push(child.clone());
        child
    }

    /// Cancel all tokens.
    pub fn cancel(&self) {
        self.primary.cancel();
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.primary.is_cancelled()
    }

    /// Wait for cancellation.
    pub async fn cancelled(&self) {
        self.primary.cancelled().await
    }
}

impl Default for CancellationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "merge_abort_signals.test.rs"]
mod tests;
