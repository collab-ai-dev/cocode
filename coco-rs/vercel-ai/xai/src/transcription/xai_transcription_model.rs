use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::TranscriptionModelV4;
use vercel_ai_provider::TranscriptionModelV4CallOptions;
use vercel_ai_provider::TranscriptionModelV4Request;
use vercel_ai_provider::TranscriptionModelV4Response;
use vercel_ai_provider::TranscriptionModelV4Result;
use vercel_ai_provider::TranscriptionModelV4StreamOptions;
use vercel_ai_provider::TranscriptionModelV4StreamResult;
use vercel_ai_provider::TranscriptionSegmentV4;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::FormData;
use vercel_ai_provider_utils::ResponseHandler as _;

use crate::xai_config::XaiConfig;
use crate::xai_error::XaiFailedResponseHandler;

use super::xai_transcription_options::XaiTranscriptionProviderOptions;
use super::xai_transcription_options::extract_xai_transcription_options;
use super::xai_transcription_stream::StreamingParams;
use super::xai_transcription_stream::build_streaming_url;
use super::xai_transcription_stream::is_known_input_audio_format;
use super::xai_transcription_stream::open_streaming_transcription;

/// Response from `POST /stt`. Mirrors `xaiTranscriptionResponseSchema`.
#[derive(Debug, Deserialize)]
pub struct XaiTranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub duration: Option<f64>,
    pub words: Option<Vec<XaiTranscriptionWord>>,
}

/// A word-level timing entry in the transcription response.
#[derive(Debug, Deserialize)]
pub struct XaiTranscriptionWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// Map an audio media type to a filename extension for the multipart upload.
/// Mirrors `mediaTypeToExtension` from `@ai-sdk/provider-utils`: the subtype
/// with a handful of special cases.
fn extension_from_media_type(media_type: &str) -> String {
    let subtype = media_type
        .to_lowercase()
        .split_once('/')
        .map(|(_, sub)| sub.to_string())
        .unwrap_or_default();
    match subtype.as_str() {
        "mpeg" => "mp3".into(),
        "x-wav" => "wav".into(),
        "opus" => "ogg".into(),
        "mp4" | "x-m4a" => "m4a".into(),
        _ => subtype,
    }
}

/// Build the scalar multipart fields (name, value) in wire order. Pure —
/// mirrors `getArgs` in `xai-transcription-model.ts`. The audio `file` part
/// is appended separately, after all of these (xAI requires `file` to be the
/// final multipart field).
pub(crate) fn plan_transcription_fields(
    xai_options: &XaiTranscriptionProviderOptions,
) -> Vec<(&'static str, String)> {
    let mut fields: Vec<(&'static str, String)> = Vec::new();

    if let Some(audio_format) = xai_options.audio_format {
        fields.push(("audio_format", audio_format.as_str().into()));
    }
    if let Some(sample_rate) = xai_options.sample_rate {
        fields.push(("sample_rate", sample_rate.to_string()));
    }
    if let Some(ref language) = xai_options.language {
        fields.push(("language", language.clone()));
    }
    if let Some(format) = xai_options.format {
        fields.push(("format", format.to_string()));
    }
    if let Some(multichannel) = xai_options.multichannel {
        fields.push(("multichannel", multichannel.to_string()));
    }
    if let Some(channels) = xai_options.channels {
        fields.push(("channels", channels.to_string()));
    }
    if let Some(diarize) = xai_options.diarize {
        fields.push(("diarize", diarize.to_string()));
    }
    if let Some(filler_words) = xai_options.filler_words {
        fields.push(("filler_words", filler_words.to_string()));
    }
    if let Some(ref keyterm) = xai_options.keyterm {
        for term in keyterm.terms() {
            fields.push(("keyterm", term));
        }
    }

    fields
}

/// xAI transcription (speech-to-text) model.
///
/// Mirrors `XaiTranscriptionModel` from `@ai-sdk/xai`, both paths:
/// - `do_transcribe`: multipart `POST /stt` with the audio as the final `file`
///   field. The endpoint takes no model field — upstream pins the model id to
///   `""`; it is carried here only as response metadata.
/// - `do_stream`: real-time WebSocket STT to `wss://…/stt`.
pub struct XaiTranscriptionModel {
    model_id: String,
    config: Arc<XaiConfig>,
}

impl XaiTranscriptionModel {
    /// Create a new xAI transcription model instance.
    pub fn new(model_id: impl Into<String>, config: Arc<XaiConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl TranscriptionModelV4 for XaiTranscriptionModel {
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
        let xai_options = extract_xai_transcription_options(&options.provider_options);

        let url = self.config.url("/stt");

        let mut form = FormData::new();
        for (name, value) in plan_transcription_fields(&xai_options) {
            form = form.text(name, value);
        }

        // xAI requires `file` to be the final multipart field.
        let ext = extension_from_media_type(&options.media_type);
        let filename = format!("audio.{ext}");
        form = form.bytes_with_mime("file", options.audio, &filename, &options.media_type);

        let mut headers = self.config.get_headers();
        if let Some(ref extra) = options.headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }

        let client = self
            .config
            .client
            .as_ref()
            .map(|c| c.as_ref().clone())
            .unwrap_or_default();

        let mut request = client.post(&url);
        for (k, v) in &headers {
            request = request.header(k, v);
        }

        let response = request.multipart(form.build()).send().await.map_err(|e| {
            AISdkError::new(format!("xAI transcription request failed: {e}")).with_cause(Box::new(
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
            // Route through the shared error handler so HTTP errors surface
            // identically to the chat / responses transports.
            let err = match XaiFailedResponseHandler
                .handle(response, &url, &Value::Null)
                .await
            {
                Ok(err) | Err(err) => err,
            };
            return Err(err);
        }

        let raw_body = response
            .text()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read transcription body: {e}")))?;
        let body_value: Value = serde_json::from_str(&raw_body)
            .map_err(|e| AISdkError::new(format!("Failed to parse transcription response: {e}")))?;
        let api_response: XaiTranscriptionResponse = serde_json::from_value(body_value.clone())
            .map_err(|e| AISdkError::new(format!("Failed to parse transcription response: {e}")))?;

        let segments: Vec<TranscriptionSegmentV4> = api_response
            .words
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|word| TranscriptionSegmentV4::new(&word.text, word.start, word.end))
            .collect();

        let mut result = TranscriptionModelV4Result::new(api_response.text)
            .with_segments(segments)
            .with_response(
                TranscriptionModelV4Response::default()
                    .with_model_id(self.model_id.clone())
                    .with_timestamp(chrono::Utc::now())
                    .with_headers(response_headers)
                    .with_body(body_value),
            )
            .with_request(TranscriptionModelV4Request::default());

        // Matches the TS `response.language || undefined`: empty strings are
        // treated as absent.
        if let Some(language) = api_response.language.filter(|l| !l.is_empty()) {
            result = result.with_language(language);
        }
        if let Some(duration) = api_response.duration {
            result = result.with_duration_in_seconds(duration);
        }

        Ok(result)
    }

    async fn do_stream(
        &self,
        options: TranscriptionModelV4StreamOptions,
    ) -> Result<TranscriptionModelV4StreamResult, AISdkError> {
        let xai_options = extract_xai_transcription_options(&options.provider_options);
        let mut warnings: Vec<Warning> = Vec::new();

        // `multichannel` requires an explicit `channels` count (matches the TS
        // `InvalidArgumentError`).
        if xai_options.multichannel == Some(true) && xai_options.channels.is_none() {
            return Err(AISdkError::new(
                "providerOptions.xai.channels is required when providerOptions.xai.multichannel is true",
            ));
        }

        // `format` (inverse text normalization) is batch-only.
        if xai_options.format.is_some() {
            warnings.push(Warning::unsupported_with_details(
                "providerOptions.xai.format",
                "xAI streaming transcription does not support format.",
            ));
        }

        // Unrecognized raw-PCM media type without an explicit `audioFormat`
        // falls back to PCM encoding — warn, matching the TS.
        if xai_options.audio_format.is_none()
            && !is_known_input_audio_format(&options.input_audio_format.media_type)
        {
            warnings.push(Warning::other(format!(
                "Unrecognized inputAudioFormat.type \"{}\"; falling back to raw PCM encoding. \
                 Use audio/pcm, audio/pcmu, or audio/pcma, or set providerOptions.xai.audioFormat explicitly.",
                options.input_audio_format.media_type
            )));
        }

        let expected_done_count = if xai_options.multichannel == Some(true) {
            xai_options.channels.unwrap_or(1).max(1) as usize
        } else {
            1
        };

        let url = build_streaming_url(
            &self.config.base_url,
            &options.input_audio_format,
            &xai_options,
        )?;

        let mut headers = self.config.get_headers();
        if let Some(ref extra) = options.headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }

        let stream = open_streaming_transcription(StreamingParams {
            url: url.clone(),
            headers,
            warnings,
            audio: options.audio,
            include_raw: options.include_raw_chunks,
            abort: options.abort_signal,
            language: xai_options.language,
            expected_done_count,
        })
        .await?;

        Ok(TranscriptionModelV4StreamResult {
            stream,
            request: Some(TranscriptionModelV4Request::default().with_body(Value::String(url))),
            response: TranscriptionModelV4Response::default().with_model_id(self.model_id.clone()),
        })
    }
}

#[cfg(test)]
#[path = "xai_transcription_model.test.rs"]
mod tests;
