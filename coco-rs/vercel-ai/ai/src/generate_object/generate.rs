//! Generate structured object from a prompt (non-streaming).

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::ResponseFormat;

use crate::error::AIError;
use crate::generate_text::build_call_options::apply_call_settings;
use crate::model::LanguageModel;
use crate::model::resolve_language_model;
use crate::prompt::CallSettings;
use crate::prompt::Prompt;
use crate::types::ProviderOptions;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

use super::ObjectGenerationMode;
use super::generate_object_result::GenerateObjectFinishEvent;
use super::generate_object_result::GenerateObjectResult;

/// Options for `generate_object`.
#[derive(Default)]
pub struct GenerateObjectOptions<T> {
    /// The model to use.
    pub model: LanguageModel,
    /// The prompt to send to the model.
    pub prompt: Prompt,
    /// The JSON schema for the output.
    pub schema: vercel_ai_provider::JSONSchema,
    /// Optional name for the schema.
    pub schema_name: Option<String>,
    /// Optional description for the schema.
    pub schema_description: Option<String>,
    /// The mode for structured output.
    pub mode: ObjectGenerationMode,
    /// Call settings.
    pub settings: CallSettings,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Maximum retries for validation failures.
    pub max_retries: Option<u32>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Callback called when generation finishes.
    #[allow(clippy::type_complexity)]
    pub on_finish: Option<Arc<dyn Fn(&GenerateObjectFinishEvent) + Send + Sync>>,
    /// Optional function to repair malformed JSON text before parsing.
    /// Called with (raw_text, error_message) when serde_json::from_str fails.
    #[allow(clippy::type_complexity)]
    pub repair_text: Option<Arc<dyn Fn(&str, &str) -> Result<String, AIError> + Send + Sync>>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Telemetry configuration.
    pub telemetry: Option<crate::telemetry::TelemetrySettings>,
    /// Phantom data for the output type.
    _phantom: std::marker::PhantomData<T>,
}

impl<T> GenerateObjectOptions<T> {
    /// Create new options with a model, prompt, and schema.
    pub fn new(
        model: impl Into<LanguageModel>,
        prompt: impl Into<Prompt>,
        schema: vercel_ai_provider::JSONSchema,
    ) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            schema,
            schema_name: None,
            schema_description: None,
            mode: ObjectGenerationMode::Auto,
            settings: CallSettings::default(),
            abort_signal: None,
            max_retries: None,
            provider_options: None,
            on_finish: None,
            repair_text: None,
            headers: None,
            telemetry: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set the schema name.
    pub fn with_schema_name(mut self, name: impl Into<String>) -> Self {
        self.schema_name = Some(name.into());
        self
    }

    /// Set the schema description.
    pub fn with_schema_description(mut self, description: impl Into<String>) -> Self {
        self.schema_description = Some(description.into());
        self
    }

    /// Set the generation mode.
    pub fn with_mode(mut self, mode: ObjectGenerationMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the call settings.
    pub fn with_settings(mut self, settings: CallSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the on_finish callback.
    pub fn with_on_finish<F>(mut self, callback: F) -> Self
    where
        F: Fn(&GenerateObjectFinishEvent) + Send + Sync + 'static,
    {
        self.on_finish = Some(Arc::new(callback));
        self
    }

    /// Set the repair_text function for repairing malformed JSON.
    pub fn with_repair_text<F>(mut self, repair: F) -> Self
    where
        F: Fn(&str, &str) -> Result<String, crate::error::AIError> + Send + Sync + 'static,
    {
        self.repair_text = Some(Arc::new(repair));
        self
    }

    /// Set the headers to include in the request.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the telemetry configuration.
    pub fn with_telemetry(mut self, telemetry: crate::telemetry::TelemetrySettings) -> Self {
        self.telemetry = Some(telemetry);
        self
    }
}

/// Generate a structured object from a prompt.
///
/// This function generates structured output that conforms to a JSON schema.
///
/// # Arguments
///
/// * `options` - The generation options including model, prompt, and schema.
///
/// # Returns
///
/// A `GenerateObjectResult<T>` containing the parsed object.
#[tracing::instrument(skip_all)]
pub async fn generate_object<T: DeserializeOwned>(
    options: GenerateObjectOptions<T>,
) -> Result<GenerateObjectResult<T>, AIError> {
    let integrations = crate::telemetry::build_integrations(options.telemetry.as_ref());

    match generate_object_inner(options).await {
        Ok(result) => Ok(result),
        Err(error) => {
            crate::telemetry::notify_error(&integrations, &error).await;
            Err(error)
        }
    }
}

async fn generate_object_inner<T: DeserializeOwned>(
    options: GenerateObjectOptions<T>,
) -> Result<GenerateObjectResult<T>, AIError> {
    let model = resolve_language_model(options.model)?;
    let messages = options.prompt.to_model_prompt();
    let mode = options.mode;

    // Build retry config
    let retry_config = options
        .max_retries
        .map(|n| RetryConfig::new().with_max_retries(n))
        .unwrap_or_default();

    let (result, raw) = match mode {
        ObjectGenerationMode::Tool => {
            let tool_name = options
                .schema_name
                .clone()
                .unwrap_or_else(|| "json_output".to_string());

            let mut func_tool =
                crate::types::LanguageModelV4FunctionTool::new(&tool_name, options.schema.clone());
            func_tool.description = Some(
                options
                    .schema_description
                    .clone()
                    .unwrap_or_else(|| "Generate structured output".to_string()),
            );

            let tool = vercel_ai_provider::LanguageModelV4Tool::function(func_tool);

            let mut call_options = LanguageModelV4CallOptions::new(messages);
            call_options.tools = Some(vec![tool]);
            call_options.tool_choice =
                Some(vercel_ai_provider::LanguageModelV4ToolChoice::required());
            apply_call_settings(&mut call_options, &options.settings);
            if let Some(ref provider_opts) = options.provider_options {
                call_options.provider_options = Some(provider_opts.clone());
            }
            if let Some(ref headers) = options.headers {
                call_options.headers = Some(headers.clone());
            }

            let result = {
                let model = model.clone();
                let abort_signal = options.abort_signal.clone();
                let call_options_ref = &call_options;
                with_retry(retry_config.clone(), abort_signal.clone(), || {
                    let model = model.clone();
                    let abort_signal = abort_signal.clone();
                    async move {
                        model
                            .do_generate(call_options_ref, abort_signal)
                            .await
                            .map_err(AIError::from)
                    }
                })
                .await?
            };

            let raw = extract_tool_call_result(&result.content, &tool_name)
                .unwrap_or_else(|| extract_text(&result.content));

            (result, raw)
        }
        _ => {
            let mut response_format = ResponseFormat::json_with_schema(options.schema.clone())
                .with_name(
                    options
                        .schema_name
                        .clone()
                        .unwrap_or_else(|| "output".to_string()),
                );

            if let Some(desc) = &options.schema_description {
                response_format = response_format.with_description(desc.clone());
            }

            let mut call_options = LanguageModelV4CallOptions::new(messages);
            call_options.response_format = Some(response_format);
            apply_call_settings(&mut call_options, &options.settings);
            if let Some(ref provider_opts) = options.provider_options {
                call_options.provider_options = Some(provider_opts.clone());
            }
            if let Some(ref headers) = options.headers {
                call_options.headers = Some(headers.clone());
            }

            let result = {
                let model = model.clone();
                let abort_signal = options.abort_signal.clone();
                let call_options_ref = &call_options;
                with_retry(retry_config, abort_signal.clone(), || {
                    let model = model.clone();
                    let abort_signal = abort_signal.clone();
                    async move {
                        model
                            .do_generate(call_options_ref, abort_signal)
                            .await
                            .map_err(AIError::from)
                    }
                })
                .await?
            };
            let raw = extract_text(&result.content);

            (result, raw)
        }
    };

    let object: T = match serde_json::from_str::<T>(&raw) {
        Ok(obj) => obj,
        Err(e) => {
            if let Some(ref repair_fn) = options.repair_text {
                let repaired = repair_fn(&raw, &e.to_string())?;
                serde_json::from_str(&repaired).map_err(|e2| {
                    AIError::SchemaValidation(format!("Failed to parse repaired JSON: {e2}"))
                })?
            } else {
                return Err(AIError::SchemaValidation(format!(
                    "Failed to parse JSON: {e}"
                )));
            }
        }
    };

    // Extract reasoning outputs from the provider result content
    let reasoning = crate::generate_text::extract_reasoning_outputs(&result.content);

    let mut gen_result = GenerateObjectResult::new(
        object,
        raw.clone(),
        result.usage.clone(),
        result.finish_reason.clone(),
    )
    .with_reasoning(reasoning)
    .with_warnings(result.warnings.clone());

    // Populate metadata from provider result
    if let Some(ref req) = result.request {
        gen_result.request = Some(crate::types::LanguageModelRequestMetadata {
            body: req.body.clone(),
        });
    }
    if let Some(ref resp) = result.response {
        gen_result.response = Some(crate::types::LanguageModelResponseMetadata {
            id: resp.id.clone(),
            timestamp: resp.timestamp,
            model_id: resp.model_id.clone(),
            headers: resp.headers.clone(),
            body: resp.body.clone(),
        });
    }
    if let Some(ref pm) = result.provider_metadata {
        gen_result.provider_metadata = Some(pm.clone());
    }

    if let Some(ref callback) = options.on_finish {
        callback(&GenerateObjectFinishEvent {
            usage: result.usage,
            finish_reason: result.finish_reason,
            raw,
            warnings: result.warnings,
        });
    }

    Ok(gen_result)
}

/// Extract the result from a tool call in the response content.
fn extract_tool_call_result(content: &[AssistantContentPart], tool_name: &str) -> Option<String> {
    for part in content {
        if let AssistantContentPart::ToolCall(tc) = part
            && tc.tool_name == tool_name
        {
            return Some(tc.input.to_string());
        }
    }
    None
}

/// Extract text from content parts.
fn extract_text(content: &[AssistantContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
#[path = "generate.test.rs"]
mod tests;
