//! Warning logging implementation.
//!
//! This module provides a warning logging system using `tracing` internally for actual logging.

use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use vercel_ai_provider::Warning;

/// Info message displayed on first warning.
pub const FIRST_WARNING_INFO_MESSAGE: &str =
    "AI SDK Warning System: To turn off warning logging, call set_log_warnings(None).";

/// Whether we've logged the info message before.
static HAS_LOGGED_BEFORE: AtomicBool = AtomicBool::new(false);

/// Global warning logger configuration.
static WARNING_LOGGER: RwLock<Option<LogWarningsFunction>> = RwLock::new(None);

/// A function for logging warnings.
///
/// You can set a custom logger using `set_log_warnings` to use it as the
/// default warning logger.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::logger::{set_log_warnings, LogWarningsFunction};
///
/// set_log_warnings(Some(LogWarningsFunction::new(|options| {
///     println!("WARNINGS: {:?}, provider: {}, model: {}",
///         options.warnings, options.provider, options.model);
/// })));
/// ```
pub struct LogWarningsFunction {
    /// The inner function.
    f: Box<dyn Fn(&LogWarningsOptions) + Send + Sync>,
}

impl LogWarningsFunction {
    /// Create a new log warnings function.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&LogWarningsOptions) + Send + Sync + 'static,
    {
        Self { f: Box::new(f) }
    }

    /// Call the function with the given options.
    pub fn call(&self, options: &LogWarningsOptions) {
        (self.f)(options)
    }
}

impl std::fmt::Debug for LogWarningsFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogWarningsFunction").finish()
    }
}

/// Options for logging warnings.
#[derive(Debug, Clone)]
pub struct LogWarningsOptions {
    /// The warnings returned by the model provider.
    pub warnings: Vec<Warning>,
    /// The provider ID used for the call.
    pub provider: String,
    /// The model ID used for the call.
    pub model: String,
}

impl LogWarningsOptions {
    /// Create new log warnings options.
    pub fn new(
        warnings: Vec<Warning>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            warnings,
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Set the global warning logger.
///
/// Pass `None` to disable warning logging.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::logger::{set_log_warnings, LogWarningsFunction};
///
/// // Enable custom logging
/// set_log_warnings(Some(LogWarningsFunction::new(|options| {
///     for warning in &options.warnings {
///         eprintln!("Warning: {:?}", warning);
///     }
/// })));
///
/// // Disable warning logging
/// set_log_warnings(None);
/// ```
#[allow(clippy::expect_used)]
pub fn set_log_warnings(logger: Option<LogWarningsFunction>) {
    let mut global_logger = WARNING_LOGGER.write().expect("lock poisoned");
    *global_logger = logger;
}

/// Format a warning object into a human-readable string.
fn format_warning(warning: &Warning, provider: &str, model: &str) -> String {
    let prefix = format!("AI SDK Warning ({provider} / {model}):");

    match warning {
        Warning::Unsupported { feature, details } => {
            let mut message = format!("{prefix} The feature \"{feature}\" is not supported.");
            if let Some(details) = details {
                message.push_str(&format!(" {details}"));
            }
            message
        }
        Warning::Compatibility { feature, details } => {
            let mut message =
                format!("{prefix} The feature \"{feature}\" is used in a compatibility mode.");
            if let Some(details) = details {
                message.push_str(&format!(" {details}"));
            }
            message
        }
        Warning::Other { message: msg } => {
            format!("{prefix} {msg}")
        }
        // Handle any future warning types
        _ => {
            format!("{prefix} {warning:?}")
        }
    }
}

/// Log warnings to the configured logger or using tracing.
///
/// The behavior can be customized via `set_log_warnings`:
/// - If set to `None` (default), warnings are logged using `tracing::warn!`.
/// - If set to a function, that function is called with the warnings.
///
/// # Arguments
///
/// * `options` - The options containing warnings and context.
pub fn log_warnings(options: &LogWarningsOptions) {
    // If the warnings array is empty, do nothing
    if options.warnings.is_empty() {
        return;
    }

    #[allow(clippy::expect_used)]
    let global_logger = WARNING_LOGGER.read().expect("lock poisoned");

    // Use the provided logger if set
    if let Some(ref logger) = *global_logger {
        logger.call(options);
        return;
    }

    // Drop the read lock before logging
    drop(global_logger);

    // Default behavior: log warnings using tracing
    // Display information note on first call
    if !HAS_LOGGED_BEFORE.load(Ordering::Relaxed) {
        HAS_LOGGED_BEFORE.store(true, Ordering::Relaxed);
        tracing::info!("{FIRST_WARNING_INFO_MESSAGE}");
    }

    for warning in &options.warnings {
        let formatted = format_warning(warning, &options.provider, &options.model);
        tracing::warn!("{}", formatted);
    }
}

/// Reset the internal logging state. Used for testing purposes.
pub fn reset_log_warnings_state() {
    HAS_LOGGED_BEFORE.store(false, Ordering::Relaxed);
}

#[cfg(test)]
#[path = "log_warnings.test.rs"]
mod tests;
