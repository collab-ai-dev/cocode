use std::collections::HashMap;
use std::env;

use coco_mcp_types::CallToolResult;
use reqwest::ClientBuilder;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use rmcp::model::CallToolResult as RmcpCallToolResult;
use serde_json::Value;

use crate::Result;
use crate::ResultExt;

pub(crate) fn convert_call_tool_result(result: RmcpCallToolResult) -> Result<CallToolResult> {
    let mut value = serde_json::to_value(result)?;
    if let Some(obj) = value.as_object_mut()
        && (obj.get("content").is_none()
            || obj.get("content").is_some_and(serde_json::Value::is_null))
    {
        obj.insert("content".to_string(), Value::Array(Vec::new()));
    }
    serde_json::from_value(value).with_ctx("failed to convert call tool result")
}

/// Convert from mcp-types to Rust SDK types.
///
/// The Rust SDK types are the same as our mcp-types crate because they are both
/// derived from the same MCP specification.
/// As a result, it should be safe to convert directly from one to the other.
pub(crate) fn convert_to_rmcp<T, U>(value: T) -> Result<U>
where
    T: serde::Serialize,
    U: serde::de::DeserializeOwned,
{
    let json = serde_json::to_value(value)?;
    Ok(serde_json::from_value(json)?)
}

/// Convert from Rust SDK types to mcp-types.
///
/// The Rust SDK types are the same as our mcp-types crate because they are both
/// derived from the same MCP specification.
/// As a result, it should be safe to convert directly from one to the other.
pub(crate) fn convert_to_mcp<T, U>(value: T) -> Result<U>
where
    T: serde::Serialize,
    U: serde::de::DeserializeOwned,
{
    let json = serde_json::to_value(value)?;
    Ok(serde_json::from_value(json)?)
}

pub(crate) fn create_env_for_mcp_server(
    extra_env: Option<HashMap<String, String>>,
    env_vars: &[String],
) -> HashMap<String, String> {
    DEFAULT_ENV_VARS
        .iter()
        .copied()
        .chain(env_vars.iter().map(String::as_str))
        .filter_map(|var| env::var(var).ok().map(|value| (var.to_string(), value)))
        .chain(extra_env.unwrap_or_default())
        .collect()
}

pub(crate) fn build_default_headers(
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();

    if let Some(static_headers) = http_headers {
        for (name, value) in static_headers {
            let header_name = match HeaderName::from_bytes(name.as_bytes()) {
                Ok(name) => name,
                Err(err) => {
                    tracing::warn!("invalid HTTP header name `{name}`: {err}");
                    continue;
                }
            };
            let header_value = match HeaderValue::from_str(value.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!("invalid HTTP header value for `{name}`: {err}");
                    continue;
                }
            };
            headers.insert(header_name, header_value);
        }
    }

    if let Some(env_headers) = env_http_headers {
        for (name, env_var) in env_headers {
            if let Ok(value) = env::var(&env_var) {
                if value.trim().is_empty() {
                    continue;
                }

                let header_name = match HeaderName::from_bytes(name.as_bytes()) {
                    Ok(name) => name,
                    Err(err) => {
                        tracing::warn!("invalid HTTP header name `{name}`: {err}");
                        continue;
                    }
                };

                let header_value = match HeaderValue::from_str(value.as_str()) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::warn!(
                            "invalid HTTP header value read from {env_var} for `{name}`: {err}"
                        );
                        continue;
                    }
                };
                headers.insert(header_name, header_value);
            }
        }
    }

    Ok(headers)
}

pub(crate) fn apply_default_headers(
    builder: ClientBuilder,
    default_headers: &HeaderMap,
) -> ClientBuilder {
    if default_headers.is_empty() {
        builder
    } else {
        builder.default_headers(default_headers.clone())
    }
}

#[cfg(unix)]
pub(crate) const DEFAULT_ENV_VARS: &[&str] = &[
    "HOME",
    "LOGNAME",
    "PATH",
    "SHELL",
    "USER",
    "__CF_USER_TEXT_ENCODING",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "TZ",
];

#[cfg(windows)]
pub(crate) const DEFAULT_ENV_VARS: &[&str] = &[
    // Core path resolution
    "PATH",
    "PATHEXT",
    // Shell and system roots
    "COMSPEC",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    // User context and profiles
    "USERNAME",
    "USERDOMAIN",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    // Program locations
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "PROGRAMDATA",
    // App data and caches
    "LOCALAPPDATA",
    "APPDATA",
    // Temp locations
    "TEMP",
    "TMP",
    // Common shells/pwsh hints
    "POWERSHELL",
    "PWSH",
];

#[cfg(test)]
#[path = "utils.test.rs"]
mod tests;
