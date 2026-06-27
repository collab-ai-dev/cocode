//! Serial job executor.
//!
//! This module provides a serial job executor that runs jobs one at a time.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Notify as TokioNotify;

/// A job to be executed.
pub type Job<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// A serial job executor.
pub struct SerialJobExecutor<T> {
    queue: Arc<Mutex<VecDeque<Job<T>>>>,
    notify: Arc<TokioNotify>,
}

impl<T: Send + 'static> SerialJobExecutor<T> {
    /// Create a new serial job executor.
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            notify: Arc::new(TokioNotify::new()),
        }
    }

    /// Submit a job to be executed.
    ///
    /// # Arguments
    ///
    /// * `job` - The job to execute.
    pub async fn submit<F>(&self, job: F)
    where
        F: FnOnce() -> T + Send + 'static,
    {
        let mut queue = self.queue.lock().await;
        queue.push_back(Box::new(job));
        self.notify.notify_one();
    }

    /// Run all pending jobs.
    ///
    /// # Returns
    ///
    /// A vector of job results.
    pub async fn run_all(&self) -> Vec<T> {
        let mut results = Vec::new();
        loop {
            let job = {
                let mut queue = self.queue.lock().await;
                queue.pop_front()
            };

            match job {
                Some(j) => results.push(j()),
                None => break,
            }
        }
        results
    }

    /// Get the number of pending jobs.
    pub async fn pending_count(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Clear all pending jobs.
    pub async fn clear(&self) {
        self.queue.lock().await.clear();
    }
}

impl<T: Send + 'static> Default for SerialJobExecutor<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> Clone for SerialJobExecutor<T> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            notify: self.notify.clone(),
        }
    }
}

/// Create a serial job executor.
///
/// # Returns
///
/// A new serial job executor.
pub fn create_serial_executor<T: Send + 'static>() -> SerialJobExecutor<T> {
    SerialJobExecutor::new()
}
