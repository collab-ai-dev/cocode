//! API request/response logging for inference calls.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use coco_types::TokenUsage;
use serde::Deserialize;
use serde::Serialize;

use crate::errors::InferenceError;
use coco_llm_types::StopReason;

// ---------------------------------------------------------------------------
// Known gateways
// ---------------------------------------------------------------------------

/// Known AI gateway proxies detectable from response headers or base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownGateway {
    Litellm,
    Helicone,
    Portkey,
    CloudflareAiGateway,
    Kong,
    Braintrust,
    Databricks,
}

impl fmt::Display for KnownGateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Litellm => "litellm",
            Self::Helicone => "helicone",
            Self::Portkey => "portkey",
            Self::CloudflareAiGateway => "cloudflare-ai-gateway",
            Self::Kong => "kong",
            Self::Braintrust => "braintrust",
            Self::Databricks => "databricks",
        };
        f.write_str(s)
    }
}

/// Header prefix fingerprints for gateway detection.
const GATEWAY_FINGERPRINTS: &[(KnownGateway, &[&str])] = &[
    (KnownGateway::Litellm, &["x-litellm-"]),
    (KnownGateway::Helicone, &["helicone-"]),
    (KnownGateway::Portkey, &["x-portkey-"]),
    (KnownGateway::CloudflareAiGateway, &["cf-aig-"]),
    (KnownGateway::Kong, &["x-kong-"]),
    (KnownGateway::Braintrust, &["x-bt-"]),
];

/// Host-suffix fingerprints for gateways that use provider-owned domains.
const GATEWAY_HOST_SUFFIXES: &[(KnownGateway, &[&str])] = &[(
    KnownGateway::Databricks,
    &[
        ".cloud.databricks.com",
        ".azuredatabricks.net",
        ".gcp.databricks.com",
    ],
)];

/// Detect a gateway from response header names and/or the base URL hostname.
pub fn detect_gateway(header_names: &[&str], base_url: Option<&str>) -> Option<KnownGateway> {
    // Check header prefixes
    for (gw, prefixes) in GATEWAY_FINGERPRINTS {
        if prefixes
            .iter()
            .any(|p| header_names.iter().any(|h| h.starts_with(p)))
        {
            return Some(*gw);
        }
    }

    // Check host suffixes by extracting hostname from URL
    if let Some(url_str) = base_url
        && let Some(host) = extract_hostname(url_str)
    {
        let host_lower = host.to_lowercase();
        for (gw, suffixes) in GATEWAY_HOST_SUFFIXES {
            if suffixes.iter().any(|s| host_lower.ends_with(s)) {
                return Some(*gw);
            }
        }
    }

    None
}

/// Extract hostname from a URL string without a full URL parser.
/// Handles `https://host:port/path` and `http://host/path`.
fn extract_hostname(url_str: &str) -> Option<&str> {
    // Skip scheme
    let after_scheme = url_str
        .find("://")
        .map(|i| &url_str[i + 3..])
        .unwrap_or(url_str);
    // Take until '/' or end
    let host_port = after_scheme.split('/').next()?;
    // Strip port
    let host = if host_port.starts_with('[') {
        // IPv6: [::1]:8080
        host_port.split(']').next().map(|s| &s[1..])
    } else {
        Some(host_port.split(':').next().unwrap_or(host_port))
    };
    host.filter(|h| !h.is_empty())
}

// ---------------------------------------------------------------------------
// Request log
// ---------------------------------------------------------------------------

/// Structured log of an outgoing API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    /// Model ID sent to the provider.
    pub model: String,
    /// Number of messages in the prompt.
    pub message_count: i64,
    /// Estimated input tokens (if available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens_estimate: Option<i64>,
    /// Temperature setting for the request.
    pub temperature: f64,
    /// Provider name for analytics.
    pub provider: String,
    /// Query source (repl, sdk, subagent, etc.).
    pub query_source: String,
    /// Whether fast mode was active.
    #[serde(default)]
    pub fast_mode: bool,
    /// Thinking type (adaptive/enabled/disabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_type: Option<String>,
    /// Effort level if specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_value: Option<String>,
}

/// Format a request log as a compact single-line summary for tracing output.
pub fn format_request_log(log: &RequestLog) -> String {
    let fast = if log.fast_mode { " [fast]" } else { "" };
    let thinking = log
        .thinking_type
        .as_deref()
        .map(|t| format!(" thinking={t}"))
        .unwrap_or_default();
    let tokens_str = log
        .input_tokens_estimate
        .map(|t| format!(" ~{t}tok"))
        .unwrap_or_default();
    format!(
        "API request: model={} msgs={}{tokens_str} temp={:.1} provider={} source={}{fast}{thinking}",
        log.model, log.message_count, log.temperature, log.provider, log.query_source,
    )
}

// ---------------------------------------------------------------------------
// Response log
// ---------------------------------------------------------------------------

/// Structured log of an API response (success).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseLog {
    /// Model that served the request (may differ from requested model).
    pub model: String,
    /// Token usage for this call.
    pub usage: TokenUsage,
    /// Request duration in milliseconds (successful attempt only).
    pub duration_ms: i64,
    /// Total duration including retries in milliseconds.
    pub duration_ms_including_retries: i64,
    /// Time to first token in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<i64>,
    /// Attempt number (1-based: 1 means first attempt succeeded).
    pub attempt: i32,
    /// Provider request ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Stop reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// Whether the client fell back from streaming to non-streaming.
    #[serde(default)]
    pub did_fallback_to_non_streaming: bool,
    /// Detected gateway proxy, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<KnownGateway>,
    /// Number of messages in the prompt.
    pub message_count: i64,
    /// Length of text content in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_content_length: Option<i64>,
    /// Length of thinking content in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_content_length: Option<i64>,
    /// Whether fast mode was active.
    #[serde(default)]
    pub fast_mode: bool,
    /// Warnings collected during the response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Format a response log as a compact single-line summary for tracing output.
pub fn format_response_log(log: &ResponseLog) -> String {
    let cache_pct = if log.usage.input_tokens.total > 0 {
        let pct = (log.usage.input_tokens.cache_read as f64 / log.usage.input_tokens.total as f64)
            * 100.0;
        format!(" cache={pct:.0}%")
    } else {
        String::new()
    };
    let ttft = log
        .ttft_ms
        .map(|t| format!(" ttft={t}ms"))
        .unwrap_or_default();
    let gateway_str = log.gateway.map(|g| format!(" via={g}")).unwrap_or_default();
    let stop = log
        .stop_reason
        .as_ref()
        .map(|s| format!(" stop={s}"))
        .unwrap_or_default();
    let retries = if log.attempt > 1 {
        format!(" retries={}", log.attempt - 1)
    } else {
        String::new()
    };
    let fast = if log.fast_mode { " [fast]" } else { "" };
    let warnings_str = if log.warnings.is_empty() {
        String::new()
    } else {
        format!(" warnings=[{}]", log.warnings.join(", "))
    };
    format!(
        "API response: model={} in={}tok out={}tok{cache_pct} {}ms{ttft}{stop} cost=${:.4}{retries}{gateway_str}{fast}{warnings_str}",
        log.model,
        log.usage.input_tokens.total,
        log.usage.output_tokens.total,
        log.duration_ms,
        log.cost_usd,
    )
}

// ---------------------------------------------------------------------------
// Error log
// ---------------------------------------------------------------------------

/// Structured log of an API error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorLog {
    /// Model that was targeted.
    pub model: String,
    /// The classified error.
    pub error_class: String,
    /// Human-readable error message.
    pub error_message: String,
    /// HTTP status code, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    /// Number of messages in the prompt.
    pub message_count: i64,
    /// Request duration in milliseconds.
    pub duration_ms: i64,
    /// Total duration including retries.
    pub duration_ms_including_retries: i64,
    /// Attempt number (1-based).
    pub attempt: i32,
    /// Provider request ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Client-generated request ID (survives timeouts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    /// Detected gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<KnownGateway>,
}

impl ErrorLog {
    /// Build an ErrorLog from an InferenceError and call metadata.
    pub fn from_inference_error(
        error: &InferenceError,
        model: &str,
        message_count: i64,
        duration_ms: i64,
        duration_ms_including_retries: i64,
        attempt: i32,
        request_id: Option<String>,
    ) -> Self {
        let status = match error {
            InferenceError::ProviderError { status, .. } => Some(*status),
            InferenceError::RateLimited { .. } => Some(429),
            InferenceError::Overloaded { .. } => Some(503),
            InferenceError::AuthenticationFailed { .. } => Some(401),
            InferenceError::InvalidRequest { .. } => Some(400),
            _ => None,
        };
        Self {
            model: model.to_string(),
            error_class: error.error_class().to_string(),
            error_message: error.to_string(),
            status,
            message_count,
            duration_ms,
            duration_ms_including_retries,
            attempt,
            request_id,
            client_request_id: None,
            gateway: None,
        }
    }
}

// ---------------------------------------------------------------------------
// API error parsing from response bodies
// ---------------------------------------------------------------------------

/// Parse an Anthropic-style error body to extract the error message.
///
/// Handles JSON bodies like `{"error": {"message": "...", "type": "..."}}` and
/// `{"message": "...", "type": "..."}`.
pub fn parse_api_error_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;

    // Try nested: { "error": { "message": "..." } }
    if let Some(msg) = value
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        return Some(msg.to_string());
    }

    // Try flat: { "message": "..." }
    if let Some(msg) = value.get("message").and_then(serde_json::Value::as_str) {
        return Some(msg.to_string());
    }

    None
}

/// Parse token counts from a "prompt too long" error message.
///
/// Handles strings like "prompt is too long: 137500 tokens > 135000 maximum".
pub fn parse_prompt_too_long_tokens(message: &str) -> Option<(i64, i64)> {
    // Pattern: "prompt is too long" ... <actual> tokens > <limit>
    let lower = message.to_lowercase();
    let idx = lower.find("prompt is too long")?;
    let rest = &message[idx..];

    // Find digits pattern: <number> tokens > <number>
    let re_like = extract_token_counts(rest)?;
    Some(re_like)
}

/// Extract (actual_tokens, limit_tokens) from text matching `<N> tokens > <M>`.
fn extract_token_counts(text: &str) -> Option<(i64, i64)> {
    let mut chars = text.char_indices().peekable();
    let mut actual: Option<i64> = None;
    let mut saw_greater = false;

    while let Some((i, c)) = chars.next() {
        if c.is_ascii_digit() && actual.is_none() {
            // Scan the full number
            let start = i;
            while chars.peek().is_some_and(|(_, ch)| ch.is_ascii_digit()) {
                chars.next();
            }
            let end = chars.peek().map_or(text.len(), |(j, _)| *j);
            let num_str = &text[start..end];
            // Check if followed by "token"
            let after = text[end..].trim_start();
            if after.starts_with("token") {
                actual = num_str.parse().ok();
            }
        } else if c == '>' && actual.is_some() {
            saw_greater = true;
        } else if c.is_ascii_digit() && saw_greater {
            let start = i;
            while chars.peek().is_some_and(|(_, ch)| ch.is_ascii_digit()) {
                chars.next();
            }
            let end = chars.peek().map_or(text.len(), |(j, _)| *j);
            let num_str = &text[start..end];
            if let Ok(limit) = num_str.parse::<i64>() {
                return actual.map(|a| (a, limit));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Timing helpers
// ---------------------------------------------------------------------------

/// Convert a Duration to milliseconds (i64).
pub fn duration_to_ms(d: Duration) -> i64 {
    d.as_millis() as i64
}

/// Build a properties map from a ResponseLog for analytics event logging.
pub fn response_log_to_properties(log: &ResponseLog) -> HashMap<String, serde_json::Value> {
    let mut props = HashMap::new();
    props.insert("model".into(), serde_json::json!(log.model));
    props.insert(
        "input_tokens".into(),
        serde_json::json!(log.usage.input_tokens.total),
    );
    props.insert(
        "output_tokens".into(),
        serde_json::json!(log.usage.output_tokens.total),
    );
    props.insert(
        "cache_read_tokens".into(),
        serde_json::json!(log.usage.input_tokens.cache_read),
    );
    props.insert(
        "cache_creation_tokens".into(),
        serde_json::json!(log.usage.input_tokens.cache_write),
    );
    props.insert("duration_ms".into(), serde_json::json!(log.duration_ms));
    props.insert("attempt".into(), serde_json::json!(log.attempt));
    props.insert("cost_usd".into(), serde_json::json!(log.cost_usd));
    props.insert("message_count".into(), serde_json::json!(log.message_count));
    props.insert("fast_mode".into(), serde_json::json!(log.fast_mode));
    if let Some(ref stop) = log.stop_reason {
        props.insert("stop_reason".into(), serde_json::json!(stop.to_string()));
    }
    if let Some(ttft) = log.ttft_ms {
        props.insert("ttft_ms".into(), serde_json::json!(ttft));
    }
    if let Some(ref gw) = log.gateway {
        props.insert("gateway".into(), serde_json::json!(gw.to_string()));
    }
    if let Some(ref rid) = log.request_id {
        props.insert("request_id".into(), serde_json::json!(rid));
    }
    props
}

#[cfg(test)]
#[path = "logging.test.rs"]
mod tests;
