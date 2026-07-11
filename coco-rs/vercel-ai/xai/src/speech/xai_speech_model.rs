use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::SpeechModelV4;
use vercel_ai_provider::SpeechModelV4CallOptions;
use vercel_ai_provider::SpeechModelV4Request;
use vercel_ai_provider::SpeechModelV4Response;
use vercel_ai_provider::SpeechModelV4Result;
use vercel_ai_provider::Warning;
use vercel_ai_provider_utils::ResponseHandler as _;

use crate::xai_config::XaiConfig;
use crate::xai_error::XaiFailedResponseHandler;

use super::xai_speech_options::XaiSpeechProviderOptions;
use super::xai_speech_options::extract_xai_speech_options;

/// Codecs accepted by the xAI TTS endpoint.
const KNOWN_SPEECH_CODECS: &[&str] = &["mp3", "wav", "pcm", "mulaw", "alaw"];

/// Map an xAI speech codec to a MIME content type.
fn codec_to_content_type(codec: &str) -> String {
    match codec {
        "wav" => "audio/wav",
        "pcm" => "audio/pcm",
        "mulaw" => "audio/mulaw",
        "alaw" => "audio/alaw",
        // "mp3" and the fallback.
        _ => "audio/mpeg",
    }
    .into()
}

/// The prepared request: JSON body and warnings.
pub(crate) struct SpeechRequestPlan {
    pub body: Value,
    pub codec: String,
    pub warnings: Vec<Warning>,
}

/// Build the `/tts` request body + warnings. Pure — mirrors `getArgs` in
/// `xai-speech-model.ts`.
pub(crate) fn plan_speech_request(
    options: &SpeechModelV4CallOptions,
    xai_options: &XaiSpeechProviderOptions,
) -> SpeechRequestPlan {
    let mut warnings: Vec<Warning> = Vec::new();

    let voice = options.voice.as_deref().unwrap_or("eve");
    let language = options.language.as_deref().unwrap_or("auto");

    let codec = match options.output_format.as_deref() {
        Some(fmt) if KNOWN_SPEECH_CODECS.contains(&fmt) => fmt.to_string(),
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

    if options.instructions.is_some() {
        warnings.push(Warning::Unsupported {
            feature: "instructions".into(),
            details: Some(
                "xAI speech models do not support the `instructions` option. \
                 Use xAI speech tags in `text` to control delivery."
                    .into(),
            ),
        });
    }

    let mut output_format = json!({ "codec": codec });
    if let Some(sample_rate) = xai_options.sample_rate {
        output_format["sample_rate"] = json!(sample_rate);
    }
    if let Some(bit_rate) = xai_options.bit_rate {
        if codec == "mp3" {
            output_format["bit_rate"] = json!(bit_rate);
        } else {
            warnings.push(Warning::Unsupported {
                feature: "providerOptions".into(),
                details: Some(
                    "xAI `bitRate` is supported only for mp3 output. It was ignored.".into(),
                ),
            });
        }
    }

    let mut body = json!({
        "text": options.text,
        "voice_id": voice,
        "language": language,
        "output_format": output_format,
    });
    if let Some(speed) = options.speed {
        body["speed"] = json!(speed);
    }
    if let Some(latency) = xai_options.optimize_streaming_latency {
        body["optimize_streaming_latency"] = json!(latency);
    }
    if let Some(normalization) = xai_options.text_normalization {
        body["text_normalization"] = json!(normalization);
    }

    SpeechRequestPlan {
        body,
        codec,
        warnings,
    }
}

/// xAI speech (text-to-speech) model.
///
/// Mirrors `XaiSpeechModel` from `@ai-sdk/xai`: `POST /tts` with a JSON body,
/// binary audio response. The endpoint takes no model field — upstream pins
/// the model id to `""`; it is carried here only as response metadata.
pub struct XaiSpeechModel {
    model_id: String,
    config: Arc<XaiConfig>,
}

impl XaiSpeechModel {
    /// Create a new xAI speech model instance.
    pub fn new(model_id: impl Into<String>, config: Arc<XaiConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl SpeechModelV4 for XaiSpeechModel {
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
        let xai_options = extract_xai_speech_options(&options.provider_options);
        let plan = plan_speech_request(&options, &xai_options);

        let url = self.config.url("/tts");
        let mut headers = self.config.get_headers();
        headers.insert("Content-Type".into(), "application/json".into());
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

        let response = request.json(&plan.body).send().await.map_err(|e| {
            AISdkError::new(format!("xAI speech request failed: {e}")).with_cause(Box::new(
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
                .handle(response, &url, &plan.body)
                .await
            {
                Ok(err) | Err(err) => err,
            };
            return Err(err);
        }

        let content_type = response_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| codec_to_content_type(&plan.codec));

        let audio = response
            .bytes()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read xAI speech response: {e}")))?;

        Ok(SpeechModelV4Result {
            audio: audio.to_vec(),
            content_type,
            warnings: plan.warnings,
            response: SpeechModelV4Response::default()
                .with_model_id(self.model_id.clone())
                .with_timestamp(chrono::Utc::now())
                .with_headers(response_headers),
            request: Some(SpeechModelV4Request::default().with_body(plan.body)),
            provider_metadata: None,
        })
    }
}

#[cfg(test)]
#[path = "xai_speech_model.test.rs"]
mod tests;
