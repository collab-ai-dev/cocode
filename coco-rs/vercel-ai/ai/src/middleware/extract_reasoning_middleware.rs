//! Extract reasoning content from model responses.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Middleware;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::language_model_middleware::WrapGenerateOptions;
use vercel_ai_provider::language_model_middleware::WrapStreamOptions;

/// Middleware that extracts reasoning content from model responses.
///
/// Some models return reasoning content interleaved with regular content.
/// This middleware extracts the reasoning into separate reasoning parts.
pub struct ExtractReasoningMiddleware {
    /// Start tag for reasoning content.
    start_tag: String,
    /// End tag for reasoning content.
    end_tag: String,
}

impl ExtractReasoningMiddleware {
    /// Create a new extract reasoning middleware.
    ///
    /// # Arguments
    /// * `start_tag` - The tag that marks the start of reasoning content.
    /// * `end_tag` - The tag that marks the end of reasoning content.
    pub fn new(start_tag: impl Into<String>, end_tag: impl Into<String>) -> Self {
        Self {
            start_tag: start_tag.into(),
            end_tag: end_tag.into(),
        }
    }
}

#[async_trait::async_trait]
impl LanguageModelV4Middleware for ExtractReasoningMiddleware {
    async fn wrap_generate(
        &self,
        options: WrapGenerateOptions,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let result = (options.do_generate)(options.params, options.abort_signal).await?;

        // Transform content to extract reasoning
        let transformed_content = self.extract_from_content(result.content.clone());

        Ok(LanguageModelV4GenerateResult {
            content: transformed_content,
            finish_reason: result.finish_reason,
            usage: result.usage,
            request: result.request,
            response: result.response,
            warnings: result.warnings,
            provider_metadata: result.provider_metadata,
        })
    }

    async fn wrap_stream(
        &self,
        options: WrapStreamOptions,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        // For streaming, we need to process the stream
        // This is a simplified implementation that passes through
        // A full implementation would buffer and transform stream parts
        (options.do_stream)(options.params, options.abort_signal).await
    }
}

impl ExtractReasoningMiddleware {
    fn extract_from_content(
        &self,
        content: Vec<vercel_ai_provider::AssistantContentPart>,
    ) -> Vec<vercel_ai_provider::AssistantContentPart> {
        let mut result = Vec::new();

        for part in content {
            match part {
                vercel_ai_provider::AssistantContentPart::Text(ref text_part) => {
                    let text = &text_part.text;
                    let extracted = self.extract_reasoning_from_text(text);

                    for (is_reasoning, content_text) in extracted {
                        if is_reasoning {
                            result.push(vercel_ai_provider::AssistantContentPart::Reasoning(
                                vercel_ai_provider::ReasoningPart {
                                    text: content_text,
                                    provider_metadata: None,
                                },
                            ));
                        } else {
                            result.push(vercel_ai_provider::AssistantContentPart::Text(
                                vercel_ai_provider::TextPart {
                                    text: content_text,
                                    provider_metadata: None,
                                },
                            ));
                        }
                    }
                }
                _ => result.push(part),
            }
        }

        result
    }

    fn extract_reasoning_from_text(&self, text: &str) -> Vec<(bool, String)> {
        let mut result = Vec::new();
        let mut remaining = text;

        while !remaining.is_empty() {
            if let Some(start_idx) = remaining.find(&self.start_tag) {
                // Add text before the start tag as regular text
                if start_idx > 0 {
                    result.push((false, remaining[..start_idx].to_string()));
                }

                // Find the end tag
                let after_start = &remaining[start_idx + self.start_tag.len()..];
                if let Some(end_idx) = after_start.find(&self.end_tag) {
                    // Add the reasoning content
                    let reasoning = &after_start[..end_idx];
                    result.push((true, reasoning.to_string()));
                    remaining = &after_start[end_idx + self.end_tag.len()..];
                } else {
                    // No end tag found, treat the rest as reasoning
                    result.push((true, after_start.to_string()));
                    break;
                }
            } else {
                // No more reasoning tags, add the rest as regular text
                result.push((false, remaining.to_string()));
                break;
            }
        }

        result
    }
}

/// Create an extract reasoning middleware.
///
/// # Arguments
/// * `start_tag` - The tag that marks the start of reasoning content.
/// * `end_tag` - The tag that marks the end of reasoning content.
pub fn extract_reasoning_middleware(
    start_tag: impl Into<String>,
    end_tag: impl Into<String>,
) -> Arc<dyn LanguageModelV4Middleware> {
    Arc::new(ExtractReasoningMiddleware::new(start_tag, end_tag))
}

#[cfg(test)]
#[path = "extract_reasoning_middleware.test.rs"]
mod tests;
