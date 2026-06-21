use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::SpeechModelV4CallOptions;
use vercel_ai_provider::SpeechModelV4Request;
use vercel_ai_provider::SpeechModelV4Response;
use vercel_ai_provider::SpeechModelV4Result;
use vercel_ai_provider::Warning;

use crate::openai_config::OpenAIConfig;

/// OpenAI Speech (text-to-speech) model.
pub struct OpenAISpeechModel {
    model_id: String,
    config: Arc<OpenAIConfig>,
}

impl OpenAISpeechModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAIConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl SpeechModelV4 for OpenAISpeechModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate_speech(
        &self,
        options: SpeechModelV4CallOptions,
    ) -> Result<SpeechModelV4Result, AISdkError> {
        let mut warnings: Vec<Warning> = Vec::new();
        let voice = options.voice.unwrap_or_else(|| "alloy".into());

        // Validate and resolve response_format (default: mp3).
        let response_format = match options.output_format.as_deref() {
            Some(fmt) if is_known_speech_format(fmt) => fmt.to_string(),
            Some(fmt) => {
                warnings.push(Warning::Unsupported {
                    feature: "outputFormat".into(),
                    details: Some(format!(
                        "Unsupported output format: {fmt}. Using mp3 instead."
                    )),
                });
                "mp3".into()
            }
            None => "mp3".into(),
        };

        let mut body = json!({
            "model": self.model_id,
            "input": options.text,
            "voice": voice,
            "response_format": response_format,
        });

        if let Some(speed) = options.speed {
            body["speed"] = json!(speed);
        }
        if let Some(ref instructions) = options.instructions {
            body["instructions"] = serde_json::Value::String(instructions.clone());
        }

        // Warn if language is provided — OpenAI speech models don't support it.
        if let Some(ref language) = options.language {
            warnings.push(Warning::Unsupported {
                feature: "language".into(),
                details: Some(format!(
                    "OpenAI speech models do not support language selection. \
                     Language parameter \"{language}\" was ignored."
                )),
            });
        }

        let url = self.config.url("/audio/speech");
        let mut headers = self.config.get_headers();
        headers.insert("Content-Type".into(), "application/json".into());

        // Merge caller-supplied headers (caller overrides defaults).
        if let Some(extra) = options.headers {
            for (k, v) in extra {
                headers.insert(k, v);
            }
        }

        let client = self
            .config
            .client
            .as_ref()
            .map(|c| c.as_ref().clone())
            .unwrap_or_default();

        let mut builder = client.post(&url);
        for (k, v) in &headers {
            builder = builder.header(k, v);
        }
        builder = builder.json(&body);

        let response = builder
            .send()
            .await
            .map_err(|e| AISdkError::new(format!("OpenAI speech request failed: {e}")))?;

        let status = response.status();

        // Capture response headers before consuming the body.
        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        if !status.is_success() {
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".into());
            return Err(AISdkError::new(format!(
                "OpenAI speech API error ({status}): {error_body}"
            )));
        }

        // Determine content type from response headers or from the requested format.
        let content_type = response_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| format_to_content_type(Some(&response_format)));

        let audio = response.bytes().await.map_err(|e| {
            AISdkError::new(format!("Failed to read OpenAI speech response body: {e}"))
        })?;

        Ok(SpeechModelV4Result {
            audio: audio.to_vec(),
            content_type,
            warnings,
            response: SpeechModelV4Response::default()
                .with_model_id(self.model_id.clone())
                .with_timestamp(chrono::Utc::now())
                .with_headers(response_headers),
            request: Some(SpeechModelV4Request::default().with_body(body)),
            provider_metadata: None,
        })
    }
}

const KNOWN_SPEECH_FORMATS: &[&str] = &["mp3", "opus", "aac", "flac", "wav", "pcm"];

fn is_known_speech_format(fmt: &str) -> bool {
    KNOWN_SPEECH_FORMATS.contains(&fmt)
}

/// Map an OpenAI response_format string to a MIME content type.
fn format_to_content_type(format: Option<&str>) -> String {
    match format {
        Some("mp3") => "audio/mpeg",
        Some("opus") => "audio/opus",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("pcm") => "audio/pcm",
        _ => "audio/mpeg",
    }
    .into()
}

#[cfg(test)]
#[path = "openai_speech_model.test.rs"]
mod tests;
