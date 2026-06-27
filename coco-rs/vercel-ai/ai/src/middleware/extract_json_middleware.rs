//! Extract JSON from model responses.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Middleware;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::language_model_middleware::WrapGenerateOptions;
use vercel_ai_provider::language_model_middleware::WrapStreamOptions;

/// Type alias for the transform function.
type TransformFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Middleware that extracts JSON from text content by stripping markdown code fences.
///
/// This is useful when using structured output with models that wrap JSON responses
/// in markdown code blocks.
pub struct ExtractJsonMiddleware {
    /// Custom transform function.
    transform: Option<TransformFn>,
}

impl ExtractJsonMiddleware {
    /// Create a new extract JSON middleware with the default transform.
    pub fn new() -> Self {
        Self { transform: None }
    }

    /// Create a new extract JSON middleware with a custom transform.
    #[allow(dead_code)]
    pub fn with_transform(transform: TransformFn) -> Self {
        Self {
            transform: Some(transform),
        }
    }
}

impl Default for ExtractJsonMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LanguageModelV4Middleware for ExtractJsonMiddleware {
    async fn wrap_generate(
        &self,
        options: WrapGenerateOptions,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let result = (options.do_generate)(options.params, options.abort_signal).await?;

        // Transform text content
        let transformed_content: Vec<vercel_ai_provider::AssistantContentPart> = result
            .content
            .into_iter()
            .map(|part| match part {
                vercel_ai_provider::AssistantContentPart::Text(text_part) => {
                    let transformed_text = self.transform_text(&text_part.text);
                    vercel_ai_provider::AssistantContentPart::Text(vercel_ai_provider::TextPart {
                        text: transformed_text,
                        provider_metadata: text_part.provider_metadata,
                    })
                }
                other => other,
            })
            .collect();

        Ok(LanguageModelV4GenerateResult {
            content: transformed_content,
            ..result
        })
    }

    async fn wrap_stream(
        &self,
        options: WrapStreamOptions,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        // For streaming, pass through (full implementation would buffer and transform)
        (options.do_stream)(options.params, options.abort_signal).await
    }
}

impl ExtractJsonMiddleware {
    fn transform_text(&self, text: &str) -> String {
        if let Some(ref transform) = self.transform {
            transform(text)
        } else {
            default_transform(text)
        }
    }
}

/// Default transform that strips markdown code fences.
fn default_transform(text: &str) -> String {
    let text = text.trim();

    // Try to strip various forms of code fences
    if text.starts_with("```json\n") {
        text.strip_prefix("```json\n")
            .and_then(|t| t.strip_suffix("\n```"))
            .or_else(|| {
                text.strip_prefix("```json\n")
                    .and_then(|t| t.strip_suffix("```"))
            })
            .unwrap_or(text)
            .trim()
            .to_string()
    } else if text.starts_with("```json") && !text.starts_with("```json\n") {
        // Handle case like ```json{"key": "value"}```
        text.strip_prefix("```json")
            .and_then(|t| t.strip_suffix("```"))
            .unwrap_or(text)
            .trim()
            .to_string()
    } else if text.starts_with("```\n") {
        text.strip_prefix("```\n")
            .and_then(|t| t.strip_suffix("\n```"))
            .or_else(|| {
                text.strip_prefix("```\n")
                    .and_then(|t| t.strip_suffix("```"))
            })
            .unwrap_or(text)
            .trim()
            .to_string()
    } else if text.starts_with("```") {
        text.strip_prefix("```")
            .and_then(|t| t.strip_suffix("```"))
            .unwrap_or(text)
            .trim()
            .to_string()
    } else {
        text.to_string()
    }
}

/// Create an extract JSON middleware with default settings.
pub fn extract_json_middleware() -> Arc<dyn LanguageModelV4Middleware> {
    Arc::new(ExtractJsonMiddleware::new())
}

#[cfg(test)]
#[path = "extract_json_middleware.test.rs"]
mod tests;
