//! Google Generative AI video model implementation.
//!
//! Uses async polling: POST to `:predictLongRunning`, then poll GET until `done: true`.
//! Response format: `generatedSamples[].video.uri` (URL-based, matching TS).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4CallOptions;
use vercel_ai_provider::VideoModelV4Result;
use vercel_ai_provider::Warning;
use vercel_ai_provider::video_model::v4::GeneratedVideo;
use vercel_ai_provider::video_model::v4::VideoData;

use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::combine_headers;
use vercel_ai_provider_utils::delay;
use vercel_ai_provider_utils::get_from_api_with_client;
use vercel_ai_provider_utils::post_json_to_api_with_client;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::get_model_path::get_model_path;
use crate::google_error::GoogleFailedResponseHandler;
use crate::google_generative_ai_video_settings::GoogleGenerativeAIVideoSettings;

/// Configuration for the Google Generative AI video model.
pub struct GoogleGenerativeAIVideoModelConfig {
    /// Provider identifier string.
    pub provider: String,
    /// Base URL for the API.
    pub base_url: String,
    /// Function to generate request headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Optional HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

/// Google Generative AI video model.
///
/// Generates videos using the Google Generative AI `:predictLongRunning` endpoint
/// with polling for completion.
pub struct GoogleGenerativeAIVideoModel {
    model_id: String,
    config: GoogleGenerativeAIVideoModelConfig,
}

impl GoogleGenerativeAIVideoModel {
    /// Create a new Google Generative AI video model.
    pub fn new(model_id: impl Into<String>, config: GoogleGenerativeAIVideoModelConfig) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Parse provider options from call options.
    fn parse_provider_options(
        options: &VideoModelV4CallOptions,
    ) -> Option<GoogleGenerativeAIVideoSettings> {
        let provider_options = options.provider_options.as_ref()?;
        let opts_map = provider_options.0.get("google")?;
        let opts_value = serde_json::to_value(opts_map).ok()?;
        serde_json::from_value(opts_value).ok()
    }

    /// Get poll interval from provider options, defaulting to 10 seconds (per Google docs).
    fn poll_interval(google_opts: &Option<GoogleGenerativeAIVideoSettings>) -> Duration {
        let ms = google_opts
            .as_ref()
            .and_then(|o| o.poll_interval_ms)
            .unwrap_or(10_000);
        Duration::from_millis(ms)
    }

    /// Get poll timeout from provider options, defaulting to 10 minutes.
    fn poll_timeout(google_opts: &Option<GoogleGenerativeAIVideoSettings>) -> Duration {
        let ms = google_opts
            .as_ref()
            .and_then(|o| o.poll_timeout_ms)
            .unwrap_or(600_000);
        Duration::from_millis(ms)
    }

    /// Map resolution dimensions to Google API resolution string.
    fn map_resolution(resolution: &str) -> &str {
        match resolution {
            "1280x720" => "720p",
            "1920x1080" => "1080p",
            "3840x2160" => "4k",
            other => other,
        }
    }
}

#[async_trait]
impl VideoModelV4 for GoogleGenerativeAIVideoModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn do_generate_video(
        &self,
        options: VideoModelV4CallOptions,
    ) -> Result<VideoModelV4Result, AISdkError> {
        let mut warnings: Vec<Warning> = Vec::new();
        let google_opts = Self::parse_provider_options(&options);

        let model_path = get_model_path(&self.model_id);
        let url = format!(
            "{}/{}:predictLongRunning",
            without_trailing_slash(&self.config.base_url),
            model_path
        );

        let headers = combine_headers(vec![Some((self.config.headers)()), options.headers.clone()]);

        let n = options.n.unwrap_or(1);

        // Build instance
        let mut instance = json!({});

        if !options.prompt.is_empty() {
            instance["prompt"] = json!(options.prompt);
        }

        // Handle image-to-video: convert to inlineData format (matching TS)
        if let Some(ref image_bytes) = options.image {
            let base64_data = base64::engine::general_purpose::STANDARD.encode(image_bytes);
            let mime_type = options.image_content_type.as_deref().unwrap_or("image/png");
            instance["image"] = json!({
                "inlineData": {
                    "mimeType": mime_type,
                    "data": base64_data,
                }
            });
        }

        // Handle referenceImages from provider options
        if let Some(ref opts) = google_opts
            && let Some(ref ref_images) = opts.reference_images
        {
            let mapped: Vec<Value> = ref_images
                .iter()
                .map(|ref_img| {
                    if let Some(ref b64) = ref_img.bytes_base64_encoded {
                        json!({
                            "inlineData": {
                                "mimeType": "image/png",
                                "data": b64,
                            }
                        })
                    } else if let Some(ref gcs) = ref_img.gcs_uri {
                        json!({ "gcsUri": gcs })
                    } else {
                        serde_json::to_value(ref_img).unwrap_or(json!({}))
                    }
                })
                .collect();
            instance["referenceImages"] = json!(mapped);
        }

        // Build parameters
        let mut parameters = json!({
            "sampleCount": n,
        });

        if let Some(ref aspect_ratio) = options.style {
            // style field is used for aspectRatio passthrough in some callers
            parameters["style"] = json!(aspect_ratio);
        }

        // Map resolution (e.g., "1280x720" -> "720p")
        if let Some(ref size) = options.size {
            let resolution_str = serde_json::to_value(size)
                .ok()
                .and_then(|v| v.as_str().map(ToString::to_string));
            if let Some(res) = resolution_str {
                parameters["resolution"] = json!(Self::map_resolution(&res));
            }
        }

        // Map duration to durationSeconds
        if let Some(ref duration) = options.duration {
            parameters["durationSeconds"] = json!(duration.seconds());
        }

        // Provider options: personGeneration, negativePrompt, and passthrough
        if let Some(ref opts) = google_opts {
            if let Some(ref pg) = opts.person_generation {
                parameters["personGeneration"] = serde_json::to_value(pg).unwrap_or(Value::Null);
            }
            if let Some(ref np) = opts.negative_prompt {
                parameters["negativePrompt"] = json!(np);
            }
        }

        let body = json!({
            "instances": [instance],
            "parameters": parameters,
        });

        // Start the long-running operation
        let operation: Value = post_json_to_api_with_client(
            &url,
            Some(headers.clone()),
            &body,
            JsonResponseHandler::new(),
            GoogleFailedResponseHandler,
            options.abort_signal.clone(),
            self.config.client.clone(),
        )
        .await?;

        // Get operation name for polling
        let operation_name = operation
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| AISdkError::new("No operation name in response"))?
            .to_string();

        // Poll for completion
        let poll_url = format!(
            "{}/{}",
            without_trailing_slash(&self.config.base_url),
            operation_name
        );

        let poll_interval = Self::poll_interval(&google_opts);
        let poll_timeout = Self::poll_timeout(&google_opts);
        let start = std::time::Instant::now();

        let mut final_status = operation;

        while final_status.get("done").and_then(Value::as_bool) != Some(true) {
            if start.elapsed() > poll_timeout {
                return Err(AISdkError::new(format!(
                    "Video generation timed out after {}ms",
                    poll_timeout.as_millis()
                )));
            }

            delay(poll_interval).await;

            // Check abort signal
            if let Some(ref signal) = options.abort_signal
                && signal.is_cancelled()
            {
                return Err(AISdkError::new("Video generation request was aborted"));
            }

            final_status = get_from_api_with_client(
                &poll_url,
                Some(headers.clone()),
                JsonResponseHandler::new(),
                GoogleFailedResponseHandler,
                options.abort_signal.clone(),
                self.config.client.clone(),
            )
            .await?;
        }

        // Check for error
        if let Some(error) = final_status.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Video generation failed");
            return Err(AISdkError::new(format!(
                "Video generation failed: {message}"
            )));
        }

        // Extract videos from response using generatedSamples[].video.uri format
        let mut videos = Vec::new();

        // Get API key from headers for URL authentication
        let resolved_headers = (self.config.headers)();
        let api_key = resolved_headers.get("x-goog-api-key");

        if let Some(response) = final_status.get("response")
            && let Some(generated_samples) = response
                .get("generateVideoResponse")
                .and_then(|gvr| gvr.get("generatedSamples"))
                .and_then(|gs| gs.as_array())
        {
            for sample in generated_samples {
                if let Some(uri) = sample
                    .get("video")
                    .and_then(|v| v.get("uri"))
                    .and_then(|u| u.as_str())
                {
                    // Append API key to download URL for authentication
                    let url_with_auth = match api_key {
                        Some(key) => {
                            let separator = if uri.contains('?') { "&" } else { "?" };
                            format!("{uri}{separator}key={key}")
                        }
                        None => uri.to_string(),
                    };

                    videos.push(GeneratedVideo {
                        data: VideoData::Url(url_with_auth),
                        content_type: Some("video/mp4".to_string()),
                    });
                }
            }
        }

        if videos.is_empty() {
            // Warn but don't error - return empty result with warnings
            warnings.push(Warning::other(format!(
                "No videos in response. Response: {}",
                serde_json::to_string(&final_status).unwrap_or_default()
            )));
        }

        Ok(VideoModelV4Result { videos })
    }
}

#[cfg(test)]
#[path = "google_generative_ai_video_model.test.rs"]
mod tests;
