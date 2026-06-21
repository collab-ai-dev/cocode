use std::time::Duration;

use crate::errors::InferenceError;

/// Max in-client retries for capacity/overload errors before letting the
/// model-runtime fallback chain take over.
const MAX_CAPACITY_RETRIES: i32 = 3;

/// Query sources that retry on a capacity cascade (503/529). Every other tagged
/// source is background and throws immediately so it doesn't amplify a saturated
/// gateway.
const FOREGROUND_529_RETRY_SOURCES: &[&str] = &[
    "repl_main_thread",
    "repl_main_thread:outputStyle:custom",
    "repl_main_thread:outputStyle:Explanatory",
    "repl_main_thread:outputStyle:Learning",
    "sdk",
    "agent:custom",
    "agent:default",
    "agent:builtin",
    "compact",
    "hook_agent",
    "hook_prompt",
    "verification_agent",
    "side_question",
    "auto_mode",
];

/// An untagged (`None`) source is treated as foreground.
fn is_foreground_source(query_source: Option<&str>) -> bool {
    match query_source {
        None => true,
        Some(src) => FOREGROUND_529_RETRY_SOURCES.contains(&src),
    }
}

/// Retry configuration for API calls.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries.
    pub max_retries: i32,
    /// Base delay for exponential backoff (milliseconds).
    pub base_delay_ms: i64,
    /// Maximum delay cap (milliseconds).
    pub max_delay_ms: i64,
    /// Jitter factor (0.0 to 1.0).
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        // DEFAULT_MAX_RETRIES = 10, base 500ms, maxDelayMs = 32000.
        // The production `RetryConfig` is built from settings via the
        // `From<ApiRetryConfig>` impl below; this default backs direct
        // `RetryConfig::default()` callers and test fixtures.
        Self {
            max_retries: 10,
            base_delay_ms: 500,
            max_delay_ms: 32_000,
            jitter_factor: 0.25,
        }
    }
}

/// Lift `coco_config::ApiRetryConfig` (settings-sourced, already
/// finalized / clamped by `ApiConfig::resolve`) into the inference
/// crate's `RetryConfig`. Fields are 1:1 — the two structs mirror each
/// other so runtime construction can pipe
/// `runtime_config.api.retry.clone().into()` directly into each slot.
impl From<coco_config::ApiRetryConfig> for RetryConfig {
    fn from(cfg: coco_config::ApiRetryConfig) -> Self {
        Self {
            max_retries: cfg.max_retries,
            base_delay_ms: cfg.base_delay_ms,
            max_delay_ms: cfg.max_delay_ms,
            jitter_factor: cfg.jitter_factor,
        }
    }
}

impl RetryConfig {
    /// Calculate delay for a given attempt number.
    /// Uses server-specified retry-after if available, else exponential backoff.
    pub fn delay_for_attempt(&self, attempt: i32, error: &InferenceError) -> Duration {
        // Use server-specified retry-after if available
        if let Some(retry_after) = error.retry_after_ms() {
            return Duration::from_millis(retry_after as u64);
        }

        // Exponential backoff: base * 2^attempt, capped at max_delay
        let delay = self
            .base_delay_ms
            .saturating_mul(2_i64.saturating_pow(attempt as u32));
        let delay = delay.min(self.max_delay_ms);

        // #136: jitter is a UNIFORM random in [0, jitter_factor*delay] so
        // concurrent clients de-correlate their retries (thundering-herd
        // mitigation). The previous fixed +25% added the same delay to
        // every client.
        let jitter = (delay as f64 * self.jitter_factor * rand::random::<f64>()) as i64;
        let jittered = delay.saturating_add(jitter);

        Duration::from_millis(jittered as u64)
    }

    /// Whether a retry should be attempted for this error at this attempt count.
    ///
    /// Only the overload cascade (503/529, [`InferenceError::Overloaded`]) is
    /// capped at [`MAX_CAPACITY_RETRIES`], so a saturated primary yields to the
    /// model-runtime fallback chain fast instead of burning the full budget
    /// in-client. All OTHER retryable errors — rate limits (429),
    /// network/timeout (408), lock/conflict (409), and generic 5xx — get the
    /// full `max_retries` (429 and `status >= 500` retry up to
    /// `DEFAULT_MAX_RETRIES`).
    pub fn should_retry(&self, attempt: i32, error: &InferenceError) -> bool {
        if !error.is_retryable() {
            return false;
        }
        let cap = if Self::is_capacity_error(error) {
            self.max_retries.min(MAX_CAPACITY_RETRIES)
        } else {
            self.max_retries
        };
        attempt < cap
    }

    /// Like [`Self::should_retry`] but throws background sources immediately on
    /// a capacity cascade: titles / suggestions / summaries / memory forks must
    /// not amplify a saturated gateway 3-10x.
    /// Foreground sources (and untagged `None`) retry per [`Self::should_retry`].
    pub fn should_retry_with_source(
        &self,
        attempt: i32,
        error: &InferenceError,
        query_source: Option<&str>,
    ) -> bool {
        if Self::is_capacity_error(error) && !is_foreground_source(query_source) {
            return false;
        }
        self.should_retry(attempt, error)
    }

    /// Overload-cascade errors (503/529) — bounded in-client so the fallback
    /// chain engages fast. Rate limits (429) are deliberately NOT capped here:
    /// they retry up to the full budget honoring `retry-after`.
    fn is_capacity_error(error: &InferenceError) -> bool {
        matches!(error, InferenceError::Overloaded { .. })
    }

    /// Check if this error is an auth failure that needs credential refresh.
    pub fn is_auth_error(error: &InferenceError) -> bool {
        matches!(
            error,
            InferenceError::AuthenticationFailed { .. }
                | InferenceError::ProviderError { status: 401, .. }
                | InferenceError::ProviderError { status: 403, .. }
        )
    }

    /// Check if this error is a rate limit that needs cooldown.
    pub fn is_rate_limit(error: &InferenceError) -> bool {
        matches!(error, InferenceError::RateLimited { .. })
    }

    /// Check if this error indicates the prompt is too long.
    pub fn is_prompt_too_long(error: &InferenceError) -> bool {
        match error {
            InferenceError::ProviderError { message, .. } => {
                message.contains("prompt is too long")
                    || message.contains("prompt_too_long")
                    || message.contains("context_length_exceeded")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "retry.test.rs"]
mod tests;
