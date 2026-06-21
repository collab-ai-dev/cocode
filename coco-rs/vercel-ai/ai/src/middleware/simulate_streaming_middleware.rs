//! Simulate streaming for non-streaming models.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4Middleware;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::ResponseMetadata;
use vercel_ai_provider::language_model_middleware::WrapStreamOptions;

/// Middleware that simulates streaming by converting a non-streaming response
/// into a stream of chunks.
///
/// This is useful for models that don't natively support streaming but you want
/// to use the streaming API.
pub struct SimulateStreamingMiddleware;

impl SimulateStreamingMiddleware {
    /// Create a new simulate streaming middleware.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SimulateStreamingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LanguageModelV4Middleware for SimulateStreamingMiddleware {
    async fn wrap_stream(
        &self,
        options: WrapStreamOptions,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        // Pass through for now - a full implementation would convert non-streaming to streaming
        (options.do_stream)(options.params, options.abort_signal).await
    }
}

/// Create a simulate streaming middleware.
pub fn simulate_streaming_middleware() -> Arc<dyn LanguageModelV4Middleware> {
    Arc::new(SimulateStreamingMiddleware::new())
}

/// Convert a generate result into simulated stream parts.
///
/// This function is used internally by the middleware and may be useful for
/// custom implementations that need to simulate streaming.
#[allow(dead_code)]
pub fn simulate_stream(result: LanguageModelV4GenerateResult) -> Vec<LanguageModelV4StreamPart> {
    let mut parts = Vec::new();
    let mut id_counter = 0u32;

    // Stream start
    parts.push(LanguageModelV4StreamPart::StreamStart {
        warnings: result.warnings.clone(),
    });

    // Response metadata
    parts.push(LanguageModelV4StreamPart::ResponseMetadata(
        ResponseMetadata::default(),
    ));

    // Content parts
    for content in result.content {
        match content {
            AssistantContentPart::Text(text_part) => {
                if !text_part.text.is_empty() {
                    let id = format!("{id_counter}");
                    parts.push(LanguageModelV4StreamPart::TextStart {
                        id: id.clone(),
                        provider_metadata: None,
                    });
                    parts.push(LanguageModelV4StreamPart::TextDelta {
                        id: id.clone(),
                        delta: text_part.text,
                        provider_metadata: None,
                    });
                    parts.push(LanguageModelV4StreamPart::TextEnd {
                        id,
                        provider_metadata: None,
                    });
                    id_counter += 1;
                }
            }
            AssistantContentPart::Reasoning(reasoning_part) => {
                let id = format!("{id_counter}");
                parts.push(LanguageModelV4StreamPart::ReasoningStart {
                    id: id.clone(),
                    provider_metadata: reasoning_part.provider_metadata.clone(),
                });
                parts.push(LanguageModelV4StreamPart::ReasoningDelta {
                    id: id.clone(),
                    delta: reasoning_part.text,
                    provider_metadata: None,
                });
                parts.push(LanguageModelV4StreamPart::ReasoningEnd {
                    id,
                    provider_metadata: None,
                });
                id_counter += 1;
            }
            _ => {
                // Skip other content types for now
            }
        }
    }

    // Finish
    parts.push(LanguageModelV4StreamPart::Finish {
        finish_reason: result.finish_reason,
        usage: result.usage,
        provider_metadata: result.provider_metadata,
    });

    parts
}

#[cfg(test)]
#[path = "simulate_streaming_middleware.test.rs"]
mod tests;
