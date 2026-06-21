use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;
use std::time::Duration;

// Re-export the snafu-generated context selectors so callers can use
// `crate::errors::ProviderErrorSnafu { ... }.build()` ergonomically.
// `#[snafu(module)]` placed selectors into the `inference_error` sub-module.
pub use inference_error::*;

/// Inference error categories.
///
/// Variant fields are positional-compatible with prior hand-rolled enum
/// (every variant adds a `location` captured by `#[snafu(implicit)]` for
/// virtual-stack debug rendering). Pattern matches in callers should use
/// `..` rest patterns.
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub), module)]
pub enum InferenceError {
    /// Authentication failure (invalid key, expired token).
    #[snafu(display("authentication failed: {message}"))]
    AuthenticationFailed {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Rate limited by provider (429).
    #[snafu(display("rate limited: {message}"))]
    RateLimited {
        retry_after_ms: Option<i64>,
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Context window exceeded.
    #[snafu(display("context window exceeded: {requested} > {max_tokens}"))]
    ContextWindowExceeded {
        max_tokens: i64,
        requested: i64,
        #[snafu(implicit)]
        location: Location,
    },

    /// Provider returned an error response.
    #[snafu(display("provider error ({status}): {message}"))]
    ProviderError {
        status: i32,
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Network error (connection, timeout).
    #[snafu(display("network error: {message}"))]
    NetworkError {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Stream interrupted.
    #[snafu(display("stream interrupted: {message}"))]
    StreamInterrupted {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Request cancelled.
    #[snafu(display("request cancelled"))]
    Cancelled {
        #[snafu(implicit)]
        location: Location,
    },

    /// Overloaded (503 / 529).
    #[snafu(display("provider overloaded"))]
    Overloaded {
        retry_after_ms: Option<i64>,
        #[snafu(implicit)]
        location: Location,
    },

    /// Invalid request (400).
    #[snafu(display("invalid request: {message}"))]
    InvalidRequest {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Model spec references a provider not registered in `RuntimeConfig`.
    #[snafu(display(
        "model spec references unknown provider `{provider}`; \
         add it to ~/.coco/providers.json or settings.providers"
    ))]
    UnknownProvider {
        provider: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Wrapped failure from a provider-specific builder — anthropic /
    /// google / openai-compatible `language_model`, or
    /// `parse_provider_options`.
    #[snafu(display("{provider} provider `{provider_name}`: {message}"))]
    ProviderBuildFailed {
        provider: &'static str,
        provider_name: String,
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl InferenceError {
    /// Suggested retry delay in milliseconds.
    pub fn retry_after_ms(&self) -> Option<i64> {
        match self {
            Self::RateLimited { retry_after_ms, .. } | Self::Overloaded { retry_after_ms, .. } => {
                *retry_after_ms
            }
            _ => None,
        }
    }

    /// Classify from an HTTP status code and body.
    pub fn from_http_status(status: i32, body: &str, retry_after: Option<i64>) -> Self {
        match status {
            400 => {
                if body.contains("context_length_exceeded")
                    || body.contains("max_tokens")
                    || body.contains("too many tokens")
                {
                    inference_error::ContextWindowExceededSnafu {
                        max_tokens: 0_i64,
                        requested: 0_i64,
                    }
                    .build()
                } else {
                    inference_error::InvalidRequestSnafu {
                        message: truncate_body(body),
                    }
                    .build()
                }
            }
            401 | 403 => inference_error::AuthenticationFailedSnafu {
                message: truncate_body(body),
            }
            .build(),
            // Request timeout (408) and lock/conflict (409) — transient,
            // retryable with the FULL backoff budget.
            408 | 409 => inference_error::NetworkSnafu {
                message: truncate_body(body),
            }
            .build(),
            429 => inference_error::RateLimitedSnafu {
                retry_after_ms: retry_after,
                message: truncate_body(body),
            }
            .build(),
            // Overload cascade (503 Service Unavailable / 529 Overloaded) — the
            // capacity-pressure bucket. Bounded in-client (MAX_CAPACITY_RETRIES)
            // so the model-runtime fallback chain engages fast rather than
            // hammering a saturated provider. Pairs 503+529 like
            // `classify_stream_message`; coco-rs applies the same bound to
            // both — a deliberate multi-provider fast-fallback choice.
            503 | 529 => inference_error::OverloadedSnafu {
                retry_after_ms: retry_after,
            }
            .build(),
            // Other 5xx (500/502/504/...) — transient server errors, retryable
            // with the FULL backoff budget (status >= 500 retries up to
            // DEFAULT_MAX_RETRIES). Keep the body for diagnostics.
            500..=599 => inference_error::NetworkSnafu {
                message: truncate_body(body),
            }
            .build(),
            // Other 4xx (404, 422, ...) are caller errors — not retryable.
            _ => inference_error::ProviderSnafu {
                status,
                message: truncate_body(body),
            }
            .build(),
        }
    }

    /// Classify a free-form error message — produced by
    /// `StreamEvent::Error.message` or `AISdkError::to_string()` — into
    /// a typed `InferenceError`. Mirrors the body-sniff fallbacks in
    /// [`Self::from_http_status`] but for the streaming path where no
    /// HTTP status is available.
    ///
    /// Returns `None` when the message doesn't match any known
    /// recoverable bucket. Callers should fall back to their default
    /// handling.
    ///
    /// This is the canonical engine-side classification site for stream
    /// errors. Higher layers (`app/query`) MUST NOT pattern-match on
    /// the raw string — the multi-provider port rule (see
    /// `coco-rs/CLAUDE.md` "Multi-Provider Boundaries") forbids
    /// Anthropic-specific keywords leaking into upper layers.
    pub fn classify_stream_message(msg: &str) -> Option<Self> {
        let lower = msg.to_ascii_lowercase();
        if lower.contains("prompt_too_long")
            || lower.contains("context_length_exceeded")
            || lower.contains("context length exceeded")
            || lower.contains("too many tokens")
        {
            return Some(
                inference_error::ContextWindowExceededSnafu {
                    max_tokens: 0_i64,
                    requested: 0_i64,
                }
                .build(),
            );
        }
        if lower.contains("overloaded_error")
            || lower.contains("provider overloaded")
            || lower.contains("status: 529")
            || lower.contains("status: 503")
            || lower.contains("(529)")
            || lower.contains("(503)")
        {
            return Some(
                inference_error::OverloadedSnafu {
                    retry_after_ms: None,
                }
                .build(),
            );
        }
        // Rate-limit vocabulary across providers: Anthropic streams
        // `rate_limit_error`; OpenAI (and OpenAI-compatible gateways that
        // deliver the 429 as an in-stream SSE error frame instead of an
        // HTTP status, e.g. the Azure front-end with `x-ms-fe-error`)
        // stream `too_many_requests` / "Too Many Requests". The HTTP-status
        // sibling `from_http_status(429, ..)` already covers the
        // status-code path; this mirrors it for the stream path where no
        // status is available.
        if lower.contains("rate limited")
            || lower.contains("rate_limit")
            || lower.contains("too_many_requests")
            || lower.contains("too many requests")
            || lower.contains("status: 429")
            || lower.contains("(429)")
        {
            return Some(
                inference_error::RateLimitedSnafu {
                    retry_after_ms: None,
                    message: msg.to_string(),
                }
                .build(),
            );
        }
        None
    }

    /// Error class for telemetry.
    pub fn error_class(&self) -> &'static str {
        match self {
            Self::AuthenticationFailed { .. } => "auth",
            Self::RateLimited { .. } => "rate_limit",
            Self::ContextWindowExceeded { .. } => "context_overflow",
            Self::ProviderError { .. } => "provider_error",
            Self::NetworkError { .. } => "network",
            Self::StreamInterrupted { .. } => "stream_interrupted",
            Self::Cancelled { .. } => "cancelled",
            Self::Overloaded { .. } => "overloaded",
            Self::InvalidRequest { .. } => "invalid_request",
            Self::UnknownProvider { .. } => "unknown_provider",
            Self::ProviderBuildFailed { .. } => "provider_build_failed",
        }
    }
}

impl ErrorExt for InferenceError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::AuthenticationFailed { .. } => StatusCode::AuthenticationFailed,
            Self::RateLimited { .. } => StatusCode::RateLimited,
            Self::ContextWindowExceeded { .. } => StatusCode::ContextWindowExceeded,
            Self::ProviderError { .. } => StatusCode::ProviderError,
            Self::NetworkError { .. } => StatusCode::NetworkError,
            Self::StreamInterrupted { .. } => StatusCode::StreamError,
            Self::Cancelled { .. } => StatusCode::Cancelled,
            Self::Overloaded { .. } => StatusCode::ServiceUnavailable,
            Self::InvalidRequest { .. } => StatusCode::InvalidRequest,
            Self::UnknownProvider { .. } => StatusCode::ProviderNotFound,
            Self::ProviderBuildFailed { .. } => StatusCode::ProviderError,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        self.retry_after_ms()
            .map(|ms| Duration::from_millis(ms as u64))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Backwards-compatible: pre-migration callers used `error.is_retryable()`
/// directly. `ErrorExt::is_retryable()` (default impl delegates to
/// `status_code().is_retryable()`) provides this; this inherent method
/// keeps `error.is_retryable()` ergonomic without `use coco_error::ErrorExt`
/// at every call site.
impl InferenceError {
    pub fn is_retryable(&self) -> bool {
        ErrorExt::is_retryable(self)
    }
}

fn truncate_body(body: &str) -> String {
    if body.len() > 500 {
        format!("{}...", &body[..500])
    } else {
        body.to_string()
    }
}

#[cfg(test)]
#[path = "errors.test.rs"]
mod tests;
