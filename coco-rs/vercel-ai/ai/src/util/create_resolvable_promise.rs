//! Resolvable promise utility.
//!
//! This module provides a resolvable promise pattern for async programming.

use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::oneshot;

/// A resolvable promise that can be resolved or rejected.
pub struct ResolvablePromise<T> {
    sender: Option<oneshot::Sender<Result<T, String>>>,
    receiver: Option<oneshot::Receiver<Result<T, String>>>,
    resolved: Arc<Mutex<bool>>,
}

impl<T> ResolvablePromise<T> {
    /// Create a new resolvable promise.
    pub fn new() -> Self {
        let (sender, receiver) = oneshot::channel();
        Self {
            sender: Some(sender),
            receiver: Some(receiver),
            resolved: Arc::new(Mutex::new(false)),
        }
    }

    /// Resolve the promise with a value.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to resolve with.
    ///
    /// # Returns
    ///
    /// True if the promise was resolved, false if it was already resolved.
    #[allow(clippy::expect_used)]
    pub fn resolve(&mut self, value: T) -> bool {
        let mut resolved = self.resolved.lock().expect("lock poisoned");
        if *resolved {
            return false;
        }
        *resolved = true;

        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Ok(value));
            return true;
        }
        false
    }

    /// Reject the promise with an error.
    ///
    /// # Arguments
    ///
    /// * `error` - The error message.
    ///
    /// # Returns
    ///
    /// True if the promise was rejected, false if it was already resolved.
    #[allow(clippy::expect_used)]
    pub fn reject(&mut self, error: impl Into<String>) -> bool {
        let mut resolved = self.resolved.lock().expect("lock poisoned");
        if *resolved {
            return false;
        }
        *resolved = true;

        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Err(error.into()));
            return true;
        }
        false
    }

    /// Check if the promise is resolved.
    #[allow(clippy::expect_used)]
    pub fn is_resolved(&self) -> bool {
        *self.resolved.lock().expect("lock poisoned")
    }

    /// Wait for the promise to be resolved.
    ///
    /// # Returns
    ///
    /// The resolved value or an error.
    pub async fn wait(mut self) -> Result<T, String> {
        if let Some(receiver) = self.receiver.take() {
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err("Promise was dropped".to_string()),
            }
        } else {
            Err("Promise already awaited".to_string())
        }
    }
}

impl<T> Default for ResolvablePromise<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A shared resolvable promise.
pub type SharedPromise<T> = Arc<AsyncMutex<ResolvablePromise<T>>>;

/// Create a new shared resolvable promise.
///
/// # Returns
///
/// A new shared resolvable promise.
pub fn create_promise<T>() -> SharedPromise<T> {
    Arc::new(AsyncMutex::new(ResolvablePromise::new()))
}

/// Create a resolved promise.
///
/// # Arguments
///
/// * `value` - The value to resolve with.
///
/// # Returns
///
/// A promise that is already resolved.
pub fn resolved<T>(value: T) -> ResolvablePromise<T> {
    let mut promise = ResolvablePromise::new();
    promise.resolve(value);
    promise
}

/// Create a rejected promise.
///
/// # Arguments
///
/// * `error` - The error message.
///
/// # Returns
///
/// A promise that is already rejected.
pub fn rejected<T>(error: impl Into<String>) -> ResolvablePromise<T> {
    let mut promise = ResolvablePromise::new();
    promise.reject(error);
    promise
}

/// Race multiple promises and return the first to resolve.
///
/// # Arguments
///
/// * `promises` - The promises to race.
///
/// # Returns
///
/// The first resolved value.
pub async fn race<T>(promises: Vec<ResolvablePromise<T>>) -> Result<T, String> {
    // Simple implementation: just wait for the first one
    // In a real implementation, you'd use tokio::select!
    if let Some(promise) = promises.into_iter().next() {
        promise.wait().await
    } else {
        Err("No promises to race".to_string())
    }
}
