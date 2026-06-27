//! Header manipulation utilities.

use std::collections::HashMap;

/// Combine multiple header maps into one.
///
/// Later headers override earlier ones for the same key.
pub fn combine_headers(headers: Vec<Option<HashMap<String, String>>>) -> HashMap<String, String> {
    let mut combined = HashMap::new();
    for header_map in headers.into_iter().flatten() {
        for (key, value) in header_map {
            combined.insert(key, value);
        }
    }
    combined
}

/// Normalize header keys to lowercase.
pub fn normalize_headers(headers: HashMap<String, String>) -> HashMap<String, String> {
    headers
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect()
}

/// Extract a header value by key (case-insensitive).
pub fn extract_header<'a>(headers: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    let key_lower = key.to_lowercase();
    headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == key_lower)
        .map(|(_, v)| v.as_str())
}

/// Create headers for a bearer token authorization.
pub fn bearer_auth(token: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("authorization".to_string(), format!("Bearer {token}"));
    headers
}

/// Create headers for JSON content type.
pub fn json_content_type() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers
}

/// Create default API headers with bearer auth and JSON content type.
pub fn default_api_headers(token: &str) -> HashMap<String, String> {
    let mut headers = bearer_auth(token);
    headers.extend(json_content_type());
    headers
}

#[cfg(test)]
#[path = "headers.test.rs"]
mod tests;
