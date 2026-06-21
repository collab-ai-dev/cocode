//! Unified status codes for error classification.
//!
//! Status code format: XX_YYY (5-digit)
//! - XX = Category (00-99)
//! - YYY = Code within category (000-999)
//!
//! Category layout:
//! - General/Core (01-09): Common, Input, IO, Network, Auth
//! - Business (10-19): Config, Provider, Resource, SystemReminder

use strum::AsRefStr;
use strum::EnumIter;
use strum::FromRepr;

/// Status code metadata.
#[derive(Debug, Clone, Copy)]
pub struct StatusMeta {
    pub retryable: bool,
    pub log_error: bool,
    pub category: StatusCategory,
}

/// Status code category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCategory {
    // ====== General/Core (00-05) ======
    /// Success (00_xxx)
    Success,
    /// Common/internal errors (01_xxx)
    Common,
    /// Parameter/validation errors (02_xxx)
    Input,
    /// IO/storage errors (03_xxx)
    IO,
    /// Network/transport errors (04_xxx)
    Network,
    /// Authentication/authorization errors (05_xxx)
    Auth,

    // ====== Business (10-13) ======
    /// Configuration errors (10_xxx)
    Config,
    /// LLM provider/model errors (11_xxx)
    Provider,
    /// Resource limits (12_xxx)
    Resource,
    /// System reminder infrastructure errors (13_xxx)
    SystemReminder,
}

macro_rules! define_status_codes {
    ($(
        $(#[$attr:meta])*
        $name:ident = $value:expr => {
            retryable: $retry:expr,
            log_error: $log:expr,
            category: $cat:ident $(,)?
        }
    ),* $(,)?) => {
        /// Status codes for error classification.
        ///
        /// Format: XX_YYY (5-digit)
        /// - XX = Category (00-99)
        /// - YYY = Code within category (000-999)
        ///
        /// Ranges:
        /// - 00_000: Success
        /// - 01_xxx: Common/Generic errors
        /// - 02_xxx: Input/Validation errors
        /// - 03_xxx: IO/Storage errors
        /// - 04_xxx: Network/Transport errors
        /// - 05_xxx: Auth errors
        /// - 10_xxx: Config errors
        /// - 11_xxx: Provider/Model errors
        /// - 12_xxx: Resource/Limit errors
        /// - 13_xxx: System reminder infrastructure errors
        #[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, EnumIter, FromRepr)]
        #[repr(i32)]
        pub enum StatusCode {
            $($(#[$attr])* $name = $value,)*
        }

        impl StatusCode {
            /// Returns the metadata for this status code.
            pub const fn meta(&self) -> StatusMeta {
                match self {
                    $(Self::$name => StatusMeta {
                        retryable: $retry,
                        log_error: $log,
                        category: StatusCategory::$cat,
                    },)*
                }
            }

            /// Returns the string name of this status code.
            pub const fn name(&self) -> &'static str {
                match self {
                    $(Self::$name => stringify!($name),)*
                }
            }
        }

        // Compile-time check for duplicate status code values
        const _: () = {
            const CODES: &[i32] = &[$($value),*];
            const fn check_unique() {
                let mut i = 0;
                while i < CODES.len() {
                    let mut j = i + 1;
                    while j < CODES.len() {
                        if CODES[i] == CODES[j] {
                            panic!("Duplicate status code value detected");
                        }
                        j += 1;
                    }
                    i += 1;
                }
            }
            check_unique();
        };
    };
}

define_status_codes! {
    // ====== Success (00_xxx) ======
    /// Operation succeeded.
    Success = 0 => { retryable: false, log_error: false, category: Success },

    // ====== Common errors (01_xxx) ======
    /// Unknown error.
    Unknown = 1_000 => { retryable: false, log_error: true, category: Common },
    /// Internal error, unexpected BUG.
    Internal = 1_001 => { retryable: true, log_error: true, category: Common },
    /// Unsupported operation.
    Unsupported = 1_002 => { retryable: false, log_error: false, category: Common },
    /// Task was cancelled.
    Cancelled = 1_003 => { retryable: false, log_error: false, category: Common },
    /// Caused by external system.
    External = 1_004 => { retryable: false, log_error: true, category: Common },

    // ====== Input/Validation errors (02_xxx) ======
    /// Invalid arguments.
    InvalidArguments = 2_000 => { retryable: false, log_error: false, category: Input },
    /// Invalid request format.
    InvalidRequest = 2_001 => { retryable: false, log_error: false, category: Input },
    /// Parse/Deserialize error.
    ParseError = 2_002 => { retryable: false, log_error: false, category: Input },
    /// Invalid JSON.
    InvalidJson = 2_003 => { retryable: false, log_error: false, category: Input },

    // ====== IO/Storage errors (03_xxx) ======
    /// IO error.
    IoError = 3_000 => { retryable: false, log_error: false, category: IO },
    /// File not found.
    FileNotFound = 3_001 => { retryable: false, log_error: false, category: IO },

    // ====== Network/Transport errors (04_xxx) ======
    /// Network error.
    NetworkError = 4_000 => { retryable: true, log_error: false, category: Network },
    /// Connection failed.
    ConnectionFailed = 4_001 => { retryable: true, log_error: false, category: Network },
    /// Service unavailable.
    ServiceUnavailable = 4_002 => { retryable: true, log_error: false, category: Network },

    // ====== Auth errors (05_xxx) ======
    /// Authentication failed (invalid credentials).
    AuthenticationFailed = 5_000 => { retryable: false, log_error: false, category: Auth },
    /// Permission denied.
    PermissionDenied = 5_001 => { retryable: false, log_error: false, category: Auth },
    /// Access denied.
    AccessDenied = 5_002 => { retryable: false, log_error: false, category: Auth },
    /// Auth header not found.
    AuthHeaderNotFound = 5_003 => { retryable: false, log_error: false, category: Auth },
    /// Invalid auth header.
    InvalidAuthHeader = 5_004 => { retryable: false, log_error: false, category: Auth },

    // ====== Config errors (10_xxx) ======
    /// Invalid configuration.
    InvalidConfig = 10_000 => { retryable: false, log_error: false, category: Config },
    /// Config file error.
    ConfigFileError = 10_001 => { retryable: false, log_error: false, category: Config },
    /// Skill is disabled by the resolved `skill_overrides` state (`off`).
    /// TS parity: errorCode 7 from the Skill tool `validateInput`
    /// (`cli_inner_pretty.js:353581-353590`).
    SkillOverrideOff = 10_002 => { retryable: false, log_error: false, category: Config },
    /// Skill is gated to `user-invocable-only` (either via
    /// `skill_overrides` or `disable-model-invocation: true`) and the
    /// user did not type `/<name>` in the current turn. The model
    /// cannot auto-invoke; the user must initiate.
    SkillOverrideUserOnlyNoTrigger = 10_003 => { retryable: false, log_error: false, category: Config },

    // ====== Provider/Model errors (11_xxx) ======
    /// Provider not found or not configured.
    ProviderNotFound = 11_000 => { retryable: false, log_error: false, category: Provider },
    /// Model not found or not available.
    ModelNotFound = 11_001 => { retryable: false, log_error: false, category: Provider },
    /// Unsupported capability for this provider/model.
    UnsupportedCapability = 11_002 => { retryable: false, log_error: false, category: Provider },
    /// Context window exceeded.
    ContextWindowExceeded = 11_003 => { retryable: false, log_error: false, category: Provider },
    /// Provider returned an error.
    ProviderError = 11_004 => { retryable: false, log_error: true, category: Provider },
    /// Streaming error.
    StreamError = 11_005 => { retryable: true, log_error: true, category: Provider },

    // ====== Resource/Limit errors (12_xxx) ======
    /// Rate limit exceeded.
    RateLimited = 12_000 => { retryable: true, log_error: false, category: Resource },
    /// Quota/Usage limit exceeded.
    QuotaExceeded = 12_001 => { retryable: false, log_error: false, category: Resource },
    /// Runtime resources exhausted.
    ResourcesExhausted = 12_002 => { retryable: true, log_error: false, category: Resource },
    /// Request timeout.
    Timeout = 12_003 => { retryable: true, log_error: false, category: Resource },
    /// Deadline exceeded.
    DeadlineExceeded = 12_004 => { retryable: false, log_error: false, category: Resource },

    // ====== System reminder errors (13_xxx) ======
    /// A reminder generator exceeded its per-turn timeout.
    ReminderGeneratorTimeout = 13_000 => { retryable: false, log_error: false, category: SystemReminder },
    /// A reminder generator returned an error from `generate()`.
    ReminderGeneratorFailed = 13_001 => { retryable: false, log_error: true, category: SystemReminder },
    /// The throttle manager lock was poisoned (another task panicked while holding it).
    ReminderThrottlePoisoned = 13_002 => { retryable: false, log_error: true, category: SystemReminder },
    /// The generator context was missing required fields for this generator.
    ReminderInvalidContext = 13_003 => { retryable: false, log_error: false, category: SystemReminder },
}

impl StatusCode {
    /// Returns true if `code` is success.
    pub fn is_success(code: i32) -> bool {
        Self::Success as i32 == code
    }

    /// Returns true if the error is retryable.
    pub const fn is_retryable(&self) -> bool {
        self.meta().retryable
    }

    /// Returns true if the error should be logged.
    pub const fn should_log_error(&self) -> bool {
        self.meta().log_error
    }

    /// Returns the category of this status code.
    pub const fn category(&self) -> StatusCategory {
        self.meta().category
    }

    /// Convert from i32.
    pub fn from_i32(value: i32) -> Option<Self> {
        Self::from_repr(value)
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
#[path = "status_code.test.rs"]
mod tests;
