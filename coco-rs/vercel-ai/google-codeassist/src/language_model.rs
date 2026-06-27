//! `LanguageModelV4` over the Code Assist transport.
//!
//! Reuses `vercel-ai-google`'s Gemini codec for the heavy lifting —
//! `get_args` builds the inner `generateContent` body and `map_response`
//! turns the response into a generate result; `create_google_stream` runs the
//! SSE/part state machine. This model only swaps transport: it wraps the body
//! in the Code Assist envelope, posts it to `cloudcode-pa` with a Bearer
//! header, and unwraps the response.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::language_model::LanguageModelV4Request;

use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::generate_id as gen_id;
use vercel_ai_provider_utils::post_json_to_api_with_client;
use vercel_ai_provider_utils::post_stream_to_api_with_client;

use vercel_ai_google::ChunkEnvelope;
use vercel_ai_google::GoogleFailedResponseHandler;
use vercel_ai_google::GoogleGenerativeAILanguageModel;
use vercel_ai_google::GoogleGenerativeAILanguageModelConfig;
use vercel_ai_google::create_google_stream;

use crate::auth::CodeAssistCreds;
use crate::auth::CodeAssistCredsSupplier;
use crate::code_assist_types::CodeAssistGenerateRequest;
use crate::code_assist_types::CodeAssistGenerateResponse;
use crate::onboarding::OnboardingState;
use crate::onboarding::auth_headers;
use crate::onboarding::run_onboarding;

/// Gemini Code Assist language model.
pub struct GoogleCodeAssistLanguageModel {
    model_id: String,
    provider_name: String,
    base_url: String,
    creds: CodeAssistCredsSupplier,
    extra_headers: HashMap<String, String>,
    client: Arc<reqwest::Client>,
    /// Reused Gemini wire codec (only `get_args` + `map_response` are called).
    inner: GoogleGenerativeAILanguageModel,
    /// Lazily-discovered project + session, cached for the model's lifetime.
    onboarding: Arc<Mutex<Option<OnboardingState>>>,
    /// Random id generator for `user_prompt_id` (request correlation).
    id_gen: Arc<dyn Fn() -> String + Send + Sync>,
}

impl GoogleCodeAssistLanguageModel {
    /// Construct a Code Assist model. `provider_name` is the public provider
    /// label; the inner codec always uses provider `"google"` so it selects the
    /// `google` provider-options namespace and metadata key.
    pub fn new(
        model_id: impl Into<String>,
        provider_name: String,
        base_url: String,
        creds: CodeAssistCredsSupplier,
        extra_headers: HashMap<String, String>,
        client: Arc<reqwest::Client>,
    ) -> Self {
        let model_id = model_id.into();
        let id_gen: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(|| gen_id("google"));
        let empty_headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync> =
            Arc::new(HashMap::new);
        let inner = GoogleGenerativeAILanguageModel::new(
            model_id.clone(),
            GoogleGenerativeAILanguageModelConfig {
                provider: "google".to_string(),
                // Unused by `get_args` / `map_response` (this model owns the
                // transport); only present to satisfy the codec config.
                base_url: base_url.clone(),
                headers: empty_headers,
                generate_id: id_gen.clone(),
                supported_urls: None,
                client: None,
            },
        );
        Self {
            model_id,
            provider_name,
            base_url,
            creds,
            extra_headers,
            client,
            inner,
            onboarding: Arc::new(Mutex::new(None)),
            id_gen,
        }
    }

    fn creds_or_err(&self) -> Result<CodeAssistCreds, AISdkError> {
        (self.creds)().ok_or_else(|| {
            AISdkError::new("Gemini Code Assist: not logged in (run `coco login gemini`)")
        })
    }

    /// Resolve (and cache) the project + session. An explicit `project_id` on
    /// the credential short-circuits onboarding; otherwise the handshake runs
    /// once and the result is cached.
    async fn ensure_state(&self, creds: &CodeAssistCreds) -> Result<OnboardingState, AISdkError> {
        {
            let guard = self.onboarding.lock().await;
            if let Some(state) = guard.as_ref() {
                return Ok(state.clone());
            }
        }
        let state = match creds.project_id.clone() {
            Some(project_id) => OnboardingState {
                project_id,
                session_id: gen_id("session"),
            },
            None => {
                run_onboarding(self.client.clone(), &self.base_url, &creds.access_token).await?
            }
        };
        let mut guard = self.onboarding.lock().await;
        // Double-check: another task may have populated it while we ran.
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }
        *guard = Some(state.clone());
        Ok(state)
    }

    fn request_headers(&self, access_token: &str) -> HashMap<String, String> {
        let mut headers = auth_headers(access_token);
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }
        headers
    }

    /// Wrap the inner `generateContent` body in the Code Assist envelope.
    /// `session_id` rides inside the inner request (jcode parity).
    fn build_envelope(&self, mut request_body: Value, state: &OnboardingState) -> Value {
        if let Value::Object(map) = &mut request_body {
            map.insert("session_id".to_string(), json!(state.session_id));
        }
        serde_json::to_value(CodeAssistGenerateRequest {
            model: self.model_id.clone(),
            project: state.project_id.clone(),
            user_prompt_id: (self.id_gen)(),
            request: request_body,
        })
        .unwrap_or(Value::Null)
    }
}

#[async_trait]
impl LanguageModelV4 for GoogleCodeAssistLanguageModel {
    fn provider(&self) -> &str {
        &self.provider_name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let creds = self.creds_or_err()?;
        let (body, _headers, warnings, _name) = self.inner.get_args(options)?;
        let state = self.ensure_state(&creds).await?;
        let envelope = self.build_envelope(body, &state);
        let url = format!("{}:generateContent", self.base_url);
        let headers = self.request_headers(&creds.access_token);

        let wrapped: CodeAssistGenerateResponse = post_json_to_api_with_client(
            &url,
            Some(headers),
            &envelope,
            JsonResponseHandler::new(),
            GoogleFailedResponseHandler,
            abort_signal,
            Some(self.client.clone()),
        )
        .await?;

        let inner_response = wrapped
            .response
            .ok_or_else(|| AISdkError::new("Code Assist: empty response envelope"))?;
        Ok(self.inner.map_response(&inner_response, envelope, warnings))
    }

    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        let creds = self.creds_or_err()?;
        let (body, _headers, warnings, _name) = self.inner.get_args(options)?;
        let include_raw = options.include_raw_chunks.unwrap_or(false);
        let state = self.ensure_state(&creds).await?;
        let envelope = self.build_envelope(body, &state);
        let url = format!("{}:streamGenerateContent?alt=sse", self.base_url);
        let headers = self.request_headers(&creds.access_token);

        let byte_stream = post_stream_to_api_with_client(
            &url,
            Some(headers),
            &envelope,
            abort_signal,
            Some(self.client.clone()),
        )
        .await?;

        let stream = create_google_stream(
            byte_stream,
            self.id_gen.clone(),
            include_raw,
            warnings,
            "google".to_string(),
            ChunkEnvelope::CodeAssistWrapped,
        );
        let mut result = LanguageModelV4StreamResult::new(stream);
        result.request = Some(LanguageModelV4Request::new().with_body(envelope));
        Ok(result)
    }
}
