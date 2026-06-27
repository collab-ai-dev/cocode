//! Generic setting loading utilities.

use std::env;

/// Load a setting from environment variables or use a default.
///
/// # Arguments
///
/// * `value` - Optional value. If provided, this is returned directly.
/// * `env_var` - The environment variable name to check.
/// * `default` - Default value if neither value nor env var is set.
///
/// # Returns
///
/// The setting value.
pub fn load_setting<T>(value: Option<T>, env_var: &str, default: T) -> T
where
    T: From<String>,
{
    // If value is provided directly, use it
    if let Some(v) = value {
        return v;
    }

    // Try to load from environment variable
    env::var(env_var).map(T::from).unwrap_or(default)
}

/// Load an optional setting from environment variables.
///
/// # Arguments
///
/// * `value` - Optional value. If provided, this is returned directly.
/// * `env_var` - The environment variable name to check.
///
/// # Returns
///
/// The setting value if found.
pub fn load_optional_setting<T>(value: Option<T>, env_var: &str) -> Option<T>
where
    T: From<String>,
{
    // If value is provided directly, use it
    if let Some(v) = value {
        return Some(v);
    }

    // Try to load from environment variable
    env::var(env_var).ok().map(T::from)
}

/// Load a boolean setting from environment variables.
///
/// Accepts "true", "1", "yes" (case-insensitive) as true values.
pub fn load_bool_setting(value: Option<bool>, env_var: &str, default: bool) -> bool {
    if let Some(v) = value {
        return v;
    }

    env::var(env_var)
        .map(|v| {
            let v = v.to_lowercase();
            v == "true" || v == "1" || v == "yes"
        })
        .unwrap_or(default)
}

/// Load a numeric setting from environment variables.
pub fn load_numeric_setting<T>(value: Option<T>, env_var: &str, default: T) -> T
where
    T: FromStringRadix + Default,
{
    if let Some(v) = value {
        return v;
    }

    env::var(env_var)
        .ok()
        .and_then(|v| T::from_string_radix(&v, 10).ok())
        .unwrap_or(default)
}

/// Trait for parsing from string with radix.
pub trait FromStringRadix: Sized {
    fn from_string_radix(s: &str, radix: u32) -> Result<Self, std::num::ParseIntError>;
}

impl FromStringRadix for i32 {
    fn from_string_radix(s: &str, radix: u32) -> Result<Self, std::num::ParseIntError> {
        i32::from_str_radix(s, radix)
    }
}

impl FromStringRadix for i64 {
    fn from_string_radix(s: &str, radix: u32) -> Result<Self, std::num::ParseIntError> {
        i64::from_str_radix(s, radix)
    }
}

impl FromStringRadix for u32 {
    fn from_string_radix(s: &str, radix: u32) -> Result<Self, std::num::ParseIntError> {
        u32::from_str_radix(s, radix)
    }
}

impl FromStringRadix for u64 {
    fn from_string_radix(s: &str, radix: u32) -> Result<Self, std::num::ParseIntError> {
        u64::from_str_radix(s, radix)
    }
}

#[cfg(test)]
#[path = "load_setting.test.rs"]
mod tests;
