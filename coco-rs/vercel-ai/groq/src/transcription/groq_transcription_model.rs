use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4CallOptions;
use vercel_ai_provider::TranscriptionModelV4Request;
use vercel_ai_provider::TranscriptionModelV4Response;
use vercel_ai_provider::TranscriptionModelV4Result;
use vercel_ai_provider::TranscriptionSegmentV4;
use vercel_ai_provider_utils::FormData;

use crate::groq_config::GroqConfig;

use super::groq_transcription_api::GroqTranscriptionResponse;
use super::groq_transcription_options::extract_transcription_options;

/// Map an audio media type to a filename extension for the multipart upload.
fn extension_from_media_type(media_type: &str) -> &str {
    match media_type {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mp3" | "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/m4a" => "m4a",
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        _ => "bin",
    }
}

/// Groq transcription (speech-to-text) model.
///
/// Mirrors `GroqTranscriptionModel` from `@ai-sdk/groq`.
pub struct GroqTranscriptionModel {
    model_id: String,
    config: Arc<GroqConfig>,
}

impl GroqTranscriptionModel {
    /// Create a new Groq transcription model instance.
    pub fn new(model_id: impl Into<String>, config: Arc<GroqConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl TranscriptionModelV4 for GroqTranscriptionModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_transcribe(
        &self,
        options: TranscriptionModelV4CallOptions,
    ) -> Result<TranscriptionModelV4Result, AISdkError> {
        let groq_opts = extract_transcription_options(&options.provider_options);

        let url = self.config.url("/audio/transcriptions");
        let config_headers = self.config.get_headers();

        let ext = extension_from_media_type(&options.media_type);
        let filename = format!("audio.{ext}");

        let mut form = FormData::new()
            .bytes_with_mime("file", options.audio, &filename, &options.media_type)
            .text("model", self.model_id.clone());

        // Groq only sends fields the caller explicitly set (no forced
        // response_format), matching the TS provider.
        if let Some(ref language) = groq_opts.language {
            form = form.text("language", language.clone());
        }
        if let Some(ref prompt) = groq_opts.prompt {
            form = form.text("prompt", prompt.clone());
        }
        if let Some(ref response_format) = groq_opts.response_format {
            form = form.text("response_format", response_format.clone());
        }
        if let Some(temperature) = groq_opts.temperature {
            form = form.text("temperature", temperature.to_string());
        }
        if let Some(ref granularities) = groq_opts.timestamp_granularities {
            for granularity in granularities {
                form = form.text("timestamp_granularities[]", granularity.clone());
            }
        }

        let client = self
            .config
            .client
            .as_ref()
            .map(|c| c.as_ref().clone())
            .unwrap_or_default();

        let mut request = client.post(&url);
        for (k, v) in &config_headers {
            request = request.header(k, v);
        }
        if let Some(ref call_headers) = options.headers {
            for (k, v) in call_headers {
                request = request.header(k, v);
            }
        }

        let response = request.multipart(form.build()).send().await.map_err(|e| {
            AISdkError::new(format!("Groq transcription request failed: {e}")).with_cause(Box::new(
                APICallError::new(e.to_string(), &url).with_retryable(e.is_timeout()),
            ))
        })?;

        let status = response.status();
        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".to_string());
            // Parse via the same typed `GroqErrorData` schema the chat path
            // uses, so both transports surface errors identically.
            let message = serde_json::from_str::<crate::groq_error::GroqErrorData>(&body)
                .map(|d| d.error.message)
                .unwrap_or_else(|_| body.clone());
            let is_retryable = status.as_u16() == 429 || status.as_u16() >= 500;
            return Err(
                AISdkError::new(format!("Groq API error ({status}): {message}")).with_cause(
                    Box::new(
                        APICallError::new(&message, &url)
                            .with_status(status.as_u16())
                            .with_response_body(&body)
                            .with_retryable(is_retryable),
                    ),
                ),
            );
        }

        let raw_body = response
            .text()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read transcription body: {e}")))?;

        let api_response: GroqTranscriptionResponse = serde_json::from_str(&raw_body)
            .map_err(|e| AISdkError::new(format!("Failed to parse transcription response: {e}")))?;

        let segments: Vec<TranscriptionSegmentV4> = api_response
            .segments
            .as_ref()
            .map(|segs| {
                segs.iter()
                    .map(|s| TranscriptionSegmentV4::new(&s.text, s.start, s.end))
                    .collect()
            })
            .unwrap_or_default();

        let body_value = serde_json::to_value(&api_response).ok();

        let mut result = TranscriptionModelV4Result::new(api_response.text)
            .with_response(
                TranscriptionModelV4Response::default()
                    .with_model_id(self.model_id.clone())
                    .with_timestamp(chrono::Utc::now())
                    .with_headers(response_headers)
                    .with_body(body_value.unwrap_or(serde_json::Value::Null)),
            )
            .with_request(TranscriptionModelV4Request::default())
            .with_segments(segments);

        if let Some(language) = api_response.language {
            result = result.with_language(language);
        }
        if let Some(duration) = api_response.duration {
            result = result.with_duration_in_seconds(duration);
        }

        Ok(result)
    }
}

#[cfg(test)]
#[path = "groq_transcription_model.test.rs"]
mod tests;
