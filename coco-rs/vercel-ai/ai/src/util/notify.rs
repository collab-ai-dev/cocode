//! Notification utility.
//!
//! This module provides a simple notification/subscription system.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::RwLock;

/// Subscribers type alias to reduce type complexity.
type Subscribers<T> = Vec<Box<dyn Fn(&T) + Send + Sync>>;

/// A notification channel.
pub struct Notify<T> {
    subscribers: RwLock<Subscribers<T>>,
    pending: RwLock<VecDeque<T>>,
}

impl<T> Notify<T> {
    /// Create a new notification channel.
    pub fn new() -> Self {
        Self {
            subscribers: RwLock::new(Vec::new()),
            pending: RwLock::new(VecDeque::new()),
        }
    }

    /// Subscribe to notifications.
    ///
    /// # Arguments
    ///
    /// * `callback` - The callback to call when notified.
    ///
    /// # Returns
    ///
    /// A subscription handle.
    #[allow(clippy::expect_used)]
    pub fn subscribe<F>(&self, callback: F)
    where
        F: Fn(&T) + Send + Sync + 'static,
    {
        let mut subscribers = self.subscribers.write().expect("lock poisoned");
        subscribers.push(Box::new(callback));
    }

    /// Notify all subscribers.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to notify.
    #[allow(clippy::expect_used)]
    pub fn notify(&self, value: T)
    where
        T: Clone,
    {
        // Store pending notification
        let mut pending = self.pending.write().expect("lock poisoned");
        pending.push_back(value.clone());
        drop(pending);

        // Notify subscribers
        let subscribers = self.subscribers.read().expect("lock poisoned");
        for callback in subscribers.iter() {
            callback(&value);
        }
    }

    /// Get pending notifications.
    ///
    /// # Returns
    ///
    /// A vector of pending notifications.
    #[allow(clippy::expect_used)]
    pub fn drain_pending(&self) -> Vec<T>
    where
        T: Clone,
    {
        let mut pending = self.pending.write().expect("lock poisoned");
        pending.drain(..).collect()
    }

    /// Clear all pending notifications.
    #[allow(clippy::expect_used)]
    pub fn clear_pending(&self) {
        let mut pending = self.pending.write().expect("lock poisoned");
        pending.clear();
    }

    /// Get the number of pending notifications.
    #[allow(clippy::expect_used)]
    pub fn pending_count(&self) -> usize {
        self.pending.read().expect("lock poisoned").len()
    }

    /// Get the number of subscribers.
    #[allow(clippy::expect_used)]
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.read().expect("lock poisoned").len()
    }
}

impl<T> Default for Notify<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Clone for Notify<T> {
    #[allow(clippy::expect_used)]
    fn clone(&self) -> Self {
        Self {
            subscribers: RwLock::new(Vec::new()), // Don't clone subscribers
            pending: RwLock::new(
                self.pending
                    .read()
                    .expect("lock poisoned")
                    .iter()
                    .cloned()
                    .collect(),
            ),
        }
    }
}

/// A thread-safe notification channel.
pub type SharedNotify<T> = Arc<Notify<T>>;

/// Create a shared notification channel.
///
/// # Returns
///
/// A new shared notification channel.
pub fn create_notify<T>() -> SharedNotify<T> {
    Arc::new(Notify::new())
}
