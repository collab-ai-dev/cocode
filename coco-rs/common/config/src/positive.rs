//! `PositiveTokens` / `PositiveCount` — bounded-positive integer newtypes.
//!
//! JSON callers naturally write `200000` (parses as `i64`); we keep
//! the wire format `i64` and validate at the type boundary via
//! `TryFrom<i64>`. Internal repr is `u32` because no production model
//! exceeds 4G tokens; choosing `u32` over `i64` makes downstream
//! `From<PositiveTokens> for u64` infallible (compile-checked) and
//! eliminates `as u64` casts across the call chain.

use crate::error::ConfigError;
use serde::Deserialize;
use serde::Serialize;
use serde::de::Deserializer;

/// Token-count metadata that must be a positive int.
/// Constructed via `try_from(i64)`. `From<PositiveTokens> for u64` is infallible.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PositiveTokens(u32);

impl PositiveTokens {
    /// Construct from a positive `u32`. Const-panics on `value == 0`
    /// so literal zero call sites surface the bug at compile time.
    /// Use `try_from(i64)` for wire-format input that may be invalid.
    pub const fn new(value: u32) -> Self {
        assert!(value > 0, "PositiveTokens::new requires value > 0");
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<i64> for PositiveTokens {
    type Error = ConfigError;
    fn try_from(value: i64) -> Result<Self, ConfigError> {
        if value <= 0 {
            return Err(ConfigError::NonPositiveTokens { value });
        }
        u32::try_from(value)
            .map(Self)
            .map_err(|_| ConfigError::NonPositiveTokens { value })
    }
}

impl From<PositiveTokens> for u64 {
    fn from(value: PositiveTokens) -> u64 {
        u64::from(value.0)
    }
}

impl From<PositiveTokens> for i64 {
    fn from(value: PositiveTokens) -> i64 {
        i64::from(value.0)
    }
}

impl<'de> Deserialize<'de> for PositiveTokens {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

/// Positive small-int (e.g. `top_k`). Same shape as `PositiveTokens`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PositiveCount(u32);

impl PositiveCount {
    /// Const-panics on `value == 0`; see [`PositiveTokens::new`].
    pub const fn new(value: u32) -> Self {
        assert!(value > 0, "PositiveCount::new requires value > 0");
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<i64> for PositiveCount {
    type Error = ConfigError;
    fn try_from(value: i64) -> Result<Self, ConfigError> {
        if value <= 0 {
            return Err(ConfigError::NonPositiveCount { value });
        }
        u32::try_from(value)
            .map(Self)
            .map_err(|_| ConfigError::NonPositiveCount { value })
    }
}

impl From<PositiveCount> for u64 {
    fn from(value: PositiveCount) -> u64 {
        u64::from(value.0)
    }
}

impl From<PositiveCount> for i64 {
    fn from(value: PositiveCount) -> i64 {
        i64::from(value.0)
    }
}

impl<'de> Deserialize<'de> for PositiveCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "positive.test.rs"]
mod tests;
