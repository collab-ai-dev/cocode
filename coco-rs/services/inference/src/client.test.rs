use std::sync::Arc;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::LlmMessage;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4StreamResult;

use super::*;

/// Simple mock model for testing — returns a fixed text response.
struct MockModel {
    response_text: String,
}

impl MockModel {
    fn new(text: &str) -> Self {
        Self {
            response_text: text.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl LanguageModelV4 for MockModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "mock-model"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, vercel_ai_provider::AISdkError> {
        Ok(LanguageModelV4GenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: self.response_text.clone(),
                provider_metadata: None,
            })],
            usage: Usage::new(10, 5),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, vercel_ai_provider::AISdkError> {
        Err(vercel_ai_provider::AISdkError::new(
            "mock does not support streaming",
        ))
    }
}

/// Mock model that always returns an error.
struct ErrorModel;

#[async_trait::async_trait]
impl LanguageModelV4 for ErrorModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "error-model"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, vercel_ai_provider::AISdkError> {
        Err(vercel_ai_provider::AISdkError::new("simulated failure"))
    }
    async fn do_stream(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, vercel_ai_provider::AISdkError> {
        Err(vercel_ai_provider::AISdkError::new("simulated failure"))
    }
}

/// Mock model that counts how many times `do_generate` is invoked, so a test
/// can prove the retry loop short-circuited before issuing any request.
struct CountingModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl LanguageModelV4 for CountingModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "counting-model"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, vercel_ai_provider::AISdkError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(LanguageModelV4GenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "ok".to_string(),
                provider_metadata: None,
            })],
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, vercel_ai_provider::AISdkError> {
        Err(vercel_ai_provider::AISdkError::new("unused"))
    }
}

fn mock_client(text: &str) -> ApiClient {
    ApiClient::with_default_fingerprint(Arc::new(MockModel::new(text)), RetryConfig::default())
}

/// A token already cancelled before the call must short-circuit the retry loop
/// at the top of the first attempt — the model is never invoked.
#[tokio::test]
async fn test_precancelled_token_short_circuits_before_request() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let client = ApiClient::with_default_fingerprint(
        Arc::new(CountingModel {
            calls: calls.clone(),
        }),
        RetryConfig::default(),
    );
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        cancel: Some(token),
        ..Default::default()
    };

    let err = client
        .query(&params)
        .await
        .expect_err("a pre-cancelled token must abort the query");
    assert!(
        matches!(err, InferenceError::Cancelled { .. }),
        "expected Cancelled, got {err:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "model must not be invoked once the token is already cancelled"
    );
}

#[tokio::test]
async fn test_client_returns_mock_text() {
    let client = mock_client("Hello from mock!");
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        ..Default::default()
    };
    let result = client.query(&params).await.expect("query should succeed");

    // Verify we got the mock response
    assert!(!result.content.is_empty());
    assert_eq!(result.usage.input_tokens.total, 10);
    assert_eq!(result.usage.output_tokens.total, 5);
    assert_eq!(result.model, "mock-model");
    assert_eq!(result.retries, 0);
    assert!(result.total_duration_ms >= 0);
}

#[tokio::test]
async fn test_client_model_id() {
    let client = mock_client("test");
    assert_eq!(client.model_id(), "mock-model");
    assert_eq!(client.provider(), "mock");
}

#[tokio::test]
async fn test_usage_accumulation() {
    let client = mock_client("test");
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        ..Default::default()
    };

    client.query(&params).await.expect("query 1");
    client.query(&params).await.expect("query 2");

    let usage = client.accumulated_usage().await;
    assert_eq!(usage.call_count, 2);
    assert_eq!(usage.total.input_tokens.total, 20);
    assert_eq!(usage.total.output_tokens.total, 10);
}

#[tokio::test]
async fn test_error_model_fails() {
    let client = ApiClient::with_default_fingerprint(
        Arc::new(ErrorModel),
        RetryConfig {
            max_retries: 0,
            ..Default::default()
        },
    );
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        ..Default::default()
    };
    let result = client.query(&params).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_provider_error_includes_provider_and_model_attribution() {
    let client = ApiClient::with_default_fingerprint(
        Arc::new(ErrorModel),
        RetryConfig {
            max_retries: 0,
            ..Default::default()
        },
    );
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        ..Default::default()
    };
    let err = client.query(&params).await.unwrap_err();
    let message = match err {
        InferenceError::ProviderError { message, .. } => message,
        other => panic!("expected ProviderError, got {other:?}"),
    };
    assert!(
        message.contains("Provider 'mock'"),
        "missing provider attribution: {message}"
    );
    assert!(
        message.contains("model 'error-model'"),
        "missing model attribution: {message}"
    );
    assert!(
        message.contains("simulated failure"),
        "missing original error: {message}"
    );
}

#[test]
fn wrap_provider_error_classifies_retryable_status_from_cause() {
    // An Anthropic-shaped error: an `AISdkError` whose cause is an
    // `APICallError` carrying the HTTP status. `wrap_provider_error` must
    // recover the status and classify a 529 as a RETRYABLE Overloaded (not the
    // old non-retryable ProviderError that killed the backoff loop for the
    // primary provider).
    let client = ApiClient::with_default_fingerprint(Arc::new(ErrorModel), RetryConfig::default());
    let api = vercel_ai_provider::APICallError::new("Overloaded", "https://api.anthropic.com")
        .with_status(529)
        .with_retryable(true);
    let sdk = vercel_ai_provider::AISdkError::new("Anthropic API error (529): Overloaded")
        .with_cause(Box::new(api));
    let err = client.wrap_provider_error(sdk);
    assert!(
        matches!(err, InferenceError::Overloaded { .. }),
        "529 should classify as Overloaded, got {err:?}",
    );
    assert!(err.is_retryable(), "529 Overloaded must be retryable");
}

#[test]
fn wrap_provider_error_opaque_no_cause_is_non_retryable() {
    // No `APICallError` cause (opaque transport/serde error) → non-retryable
    // ProviderError, so the backoff loop doesn't spin on unknown errors.
    let client = ApiClient::with_default_fingerprint(Arc::new(ErrorModel), RetryConfig::default());
    let sdk = vercel_ai_provider::AISdkError::new("some opaque failure");
    let err = client.wrap_provider_error(sdk);
    assert!(
        matches!(err, InferenceError::ProviderError { .. }),
        "opaque error should stay ProviderError, got {err:?}",
    );
    assert!(!err.is_retryable());
}

#[test]
fn stop_reason_is_normal_covers_happy_path() {
    for normal in [
        coco_llm_types::StopReason::EndTurn,
        coco_llm_types::StopReason::StopSequence,
        coco_llm_types::StopReason::ToolUse,
    ] {
        assert!(normal.is_normal(), "{normal:?} should be normal");
        assert!(!normal.is_abnormal());
    }
}

#[test]
fn stop_reason_flags_truncation_and_filter() {
    for abnormal in [
        coco_llm_types::StopReason::MaxTokens,
        coco_llm_types::StopReason::ContextWindowExceeded,
        coco_llm_types::StopReason::ContentFilter,
        coco_llm_types::StopReason::Error,
        coco_llm_types::StopReason::Other,
    ] {
        assert!(abnormal.is_abnormal(), "{abnormal:?} should be abnormal");
        assert!(!abnormal.is_normal());
    }
}

/// Model that returns a 401 (with a downcastable `APICallError`) for the first
/// `fail_calls` invocations, then succeeds. Drives the reactive-401 path.
struct FlakyAuthModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    fail_calls: usize,
}

#[async_trait::async_trait]
impl LanguageModelV4 for FlakyAuthModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "flaky-auth"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, vercel_ai_provider::AISdkError> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < self.fail_calls {
            return Err(
                vercel_ai_provider::AISdkError::new("unauthorized").with_cause(Box::new(
                    vercel_ai_provider::APICallError::new("unauthorized", "https://x")
                        .with_status(401),
                )),
            );
        }
        Ok(LanguageModelV4GenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "recovered".to_string(),
                provider_metadata: None,
            })],
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        _options: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, vercel_ai_provider::AISdkError> {
        Err(vercel_ai_provider::AISdkError::new("unsupported"))
    }
}

#[tokio::test]
async fn reactive_401_refreshes_then_retries_and_succeeds() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(FlakyAuthModel {
        calls: calls.clone(),
        fail_calls: 1,
    });
    let refreshes = Arc::new(AtomicUsize::new(0));
    let r2 = refreshes.clone();
    let hook: crate::credentials::RefreshHook = Arc::new(move || {
        r2.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client =
        ApiClient::with_default_fingerprint(model, RetryConfig::default()).with_refresh_hook(hook);
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        ..Default::default()
    };
    let result = client.query(&params).await.expect("recovers after refresh");
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        1,
        "refresh fires exactly once"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2, "model: 401 then success");
    assert!(!result.content.is_empty());
}

#[tokio::test]
async fn reactive_401_gives_up_when_refresh_fails() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(FlakyAuthModel {
        calls: calls.clone(),
        fail_calls: usize::MAX, // always 401
    });
    let refreshes = Arc::new(AtomicUsize::new(0));
    let r2 = refreshes.clone();
    // Refresh reports failure (e.g. not actually logged in) → no recovery.
    let hook: crate::credentials::RefreshHook = Arc::new(move || {
        r2.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { false })
    });
    let client =
        ApiClient::with_default_fingerprint(model, RetryConfig::default()).with_refresh_hook(hook);
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("hi")],
        max_tokens: Some(100),
        ..Default::default()
    };
    let err = client
        .query(&params)
        .await
        .expect_err("auth error must surface");
    assert!(
        crate::retry::RetryConfig::is_auth_error(&err),
        "classified as auth: {err}"
    );
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        1,
        "refresh attempted once"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no retry when refresh fails"
    );
}
