//! Error extension traits.
//!
//! This module provides:
//! - [`StackError`] — virtual stack trace walking (auto-implemented by `#[stack_trace_debug]`)
//! - [`ErrorExt`] — unified error handling with status codes, retry semantics,
//!   and user-friendly messages
//!
//! # Example
//!
//! ```ignore
//! use coco_error::{ErrorExt, StatusCode, stack_trace_debug};
//! use snafu::Snafu;
//!
//! #[stack_trace_debug]
//! #[derive(Snafu)]
//! pub enum MyError {
//!     #[snafu(display("Network error"))]
//!     Network {
//!         #[snafu(source)]
//!         error: reqwest::Error,
//!         #[snafu(implicit)]
//!         location: Location,
//!     },
//! }
//!
//! impl ErrorExt for MyError {
//!     fn status_code(&self) -> StatusCode {
//!         match self {
//!             Self::Network { .. } => StatusCode::NetworkError,
//!         }
//!     }
//!     fn as_any(&self) -> &dyn std::any::Any { self }
//! }
//! ```

use crate::StatusCode;
use std::any::Any;
use std::time::Duration;

/// Trait for errors that support virtual stack trace walking.
///
/// Automatically implemented by `#[stack_trace_debug]` proc macro.
/// Each error layer records its own `Location` (captured at creation time via
/// `#[snafu(implicit)]`), so the resulting trace shows the *actual* call site
/// for every frame.
pub trait StackError: std::error::Error {
    /// Format this error as a stack trace frame and recurse into sources.
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>);

    /// Return the next error in the StackError chain (internal sources only).
    fn next(&self) -> Option<&dyn StackError>;

    /// Walk to the last (innermost) error in the StackError chain.
    fn last(&self) -> &dyn StackError
    where
        Self: Sized,
    {
        let Some(mut result) = self.next() else {
            return self;
        };
        while let Some(err) = result.next() {
            result = err;
        }
        result
    }
}

/// Extension trait for errors with status code and retryability.
///
/// All error types in coco-rs should implement this trait to provide:
/// - Unified status code classification
/// - Retry semantics (is_retryable, retry_after)
/// - User-friendly output messages
///
/// # Implementing for Nested Errors
///
/// When your error wraps another error that implements `ErrorExt`,
/// delegate to the source's `status_code()`:
///
/// ```ignore
/// fn status_code(&self) -> StatusCode {
///     match self {
///         Self::Upstream { source, .. } => source.status_code(),
///         Self::Local { .. } => StatusCode::Internal,
///     }
/// }
/// ```
pub trait ErrorExt: StackError {
    /// Returns the status code for this error.
    ///
    /// Override this to provide appropriate classification.
    /// Default returns `StatusCode::Unknown`.
    fn status_code(&self) -> StatusCode {
        StatusCode::Unknown
    }

    /// Returns true if this error is retryable.
    ///
    /// By default, delegates to `status_code().is_retryable()`.
    /// Override for custom retry logic.
    fn is_retryable(&self) -> bool {
        self.status_code().is_retryable()
    }

    /// Returns the retry delay if applicable.
    ///
    /// For rate-limited errors, return the suggested wait duration.
    fn retry_after(&self) -> Option<Duration> {
        None
    }

    /// Returns a user-friendly error message.
    ///
    ///
    /// Global scheme (Greptime-style):
    /// `KIND - REASON ([EXTERNAL CAUSE])`
    ///
    /// - `KIND`: `status_code().name()`
    /// - `REASON`: current error Display
    /// - `EXTERNAL CAUSE` (optional): the innermost source Display
    fn output_msg(&self) -> String {
        let kind = self.status_code().name();
        let reason = self.to_string();

        // Walk to the innermost source error.
        // We intentionally avoid system backtraces and only use the error chain.
        let mut last_source: Option<&(dyn std::error::Error + 'static)> = None;
        let mut cur: Option<&(dyn std::error::Error + 'static)> = self.source();
        while let Some(err) = cur {
            last_source = Some(err);
            cur = err.source();
        }

        match last_source {
            Some(src) => {
                let cause = src.to_string();
                if cause.is_empty() || cause == reason {
                    format!("{kind} - {reason}")
                } else {
                    format!("{kind} - {reason} ({cause})")
                }
            }
            None => format!("{kind} - {reason}"),
        }
    }

    /// Returns self as Any for downcasting.
    fn as_any(&self) -> &dyn Any;
}

/// A boxed error that implements `ErrorExt`.
///
/// Use this to wrap external errors or for type erasure.
pub type BoxedError = Box<dyn ErrorExt + Send + Sync>;

/// A sized wrapper for [`BoxedError`] so it can be used as a SNAFU `source`.
///
/// Why this exists:
/// - `BoxedError` is a trait object (`Box<dyn ErrorExt + Send + Sync>`)
/// - `snafu`'s derive wants a concrete `std::error::Error` source type that
///   satisfies its internal `AsErrorSource` bounds.
/// - Using `BoxedError` directly as `#[snafu(source)]` does not compile.
///
/// This wrapper keeps the original error chain + status code semantics,
/// and allows `#[snafu(source(from(BoxedError, BoxedErrorSource::new)))]`.
#[doc(hidden)]
#[derive(Debug)]
pub struct BoxedErrorSource {
    inner: BoxedError,
}

impl BoxedErrorSource {
    pub fn new(inner: BoxedError) -> Self {
        Self { inner }
    }

    pub fn status_code(&self) -> StatusCode {
        self.inner.status_code()
    }
}

impl std::fmt::Display for BoxedErrorSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for BoxedErrorSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

impl StackError for BoxedErrorSource {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        self.inner.debug_fmt(layer, buf)
    }

    fn next(&self) -> Option<&dyn StackError> {
        self.inner.next()
    }
}

/// Box an internal error that already implements [`ErrorExt`].
///
/// This is the preferred boundary conversion: it preserves the concrete error
/// type (so `Debug` can render `#[stack_trace_debug]` virtual stacks) and avoids
/// erasing semantics into string-only errors.
pub fn boxed_err<E>(error: E) -> BoxedError
where
    E: ErrorExt + Send + Sync + 'static,
{
    Box::new(error)
}

/// Wraps any `std::error::Error` into a `BoxedError` with the given status code.
pub fn boxed<E>(error: E, status_code: StatusCode) -> BoxedError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(PlainError {
        message: error.to_string(),
        status_code,
        source: Some(Box::new(error)),
    })
}

/// A simple error type for wrapping external errors.
#[derive(Debug)]
pub struct PlainError {
    message: String,
    status_code: StatusCode,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl PlainError {
    /// Creates a new PlainError with the given message and status code.
    pub fn new(message: impl Into<String>, status_code: StatusCode) -> Self {
        Self {
            message: message.into(),
            status_code,
            source: None,
        }
    }
}

impl std::fmt::Display for PlainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for PlainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl StackError for PlainError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {}", self.message));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for PlainError {
    fn status_code(&self) -> StatusCode {
        self.status_code
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
#[path = "ext.test.rs"]
mod tests;
