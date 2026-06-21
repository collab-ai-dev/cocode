//! Remove trailing slash from URLs and paths.
//!
//! This module provides utilities for normalizing URLs and paths by removing
//! trailing slashes.

/// Remove trailing slash from a path or URL.
///
/// Returns the input without a trailing slash. If the input is just "/",
/// returns "/" unchanged.
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::without_trailing_slash;
///
/// assert_eq!(without_trailing_slash("https://example.com/"), "https://example.com");
/// assert_eq!(without_trailing_slash("https://example.com/api/"), "https://example.com/api");
/// assert_eq!(without_trailing_slash("/path/to/resource/"), "/path/to/resource");
/// assert_eq!(without_trailing_slash("/"), "/");
/// ```
pub fn without_trailing_slash(s: &str) -> &str {
    if s == "/" {
        return s;
    }
    s.strip_suffix('/').unwrap_or(s)
}

/// Add trailing slash to a path or URL.
///
/// Returns the input with a trailing slash. If the input already has a
/// trailing slash, returns it unchanged.
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::with_trailing_slash;
///
/// assert_eq!(with_trailing_slash("https://example.com"), "https://example.com/");
/// assert_eq!(with_trailing_slash("https://example.com/"), "https://example.com/");
/// assert_eq!(with_trailing_slash("/path"), "/path/");
/// ```
pub fn with_trailing_slash(s: &str) -> String {
    if s.ends_with('/') {
        s.to_string()
    } else {
        format!("{s}/")
    }
}

/// Normalize a URL by removing trailing slash.
///
/// This is an alias for [`without_trailing_slash`] with a more specific name
/// for URL contexts.
pub fn normalize_url(url: &str) -> &str {
    without_trailing_slash(url)
}

#[cfg(test)]
#[path = "without_trailing_slash.test.rs"]
mod tests;
