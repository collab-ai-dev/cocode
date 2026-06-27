//! `RedactedSecret` — type-level Debug guard for credentials.
//!
//! Wraps a String secret so it never round-trips through `Debug`,
//! `Display`, or `format!`. The single audit point for unwrapping
//! the inner value is `.expose()` — `grep -r "expose()" coco-rs/`
//! enumerates every place a secret leaves the type.
//!
//! Defence-in-depth: works even if `secret-redact` (a string-pattern
//! post-processor at log sinks) misses a leak point — panics, snafu
//! cause chains, assertion-failure formatters, etc. all flow through
//! `Debug`/`Display`.

use serde::Deserialize;
use serde::Serialize;
use std::fmt;

/// Secret string that never round-trips through `Debug` / `Display` / `format!`.
/// Use `.expose()` at the single call-site that reads the underlying secret.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct RedactedSecret(String);

impl RedactedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Single audit point — every site grepable as `.expose()`.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedSecret(<redacted>)")
    }
}

impl fmt::Display for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl From<String> for RedactedSecret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RedactedSecret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[cfg(test)]
#[path = "secret.test.rs"]
mod tests;
