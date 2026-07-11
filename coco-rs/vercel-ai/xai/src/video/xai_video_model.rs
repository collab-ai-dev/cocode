use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4CallOptions;
use vercel_ai_provider::VideoModelV4Result;
use vercel_ai_provider::video_model::v4::GeneratedVideo;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::delay;
use vercel_ai_provider_utils::get_from_api_with_client;
use vercel_ai_provider_utils::merge_json_value;
use vercel_ai_provider_utils::post_json_to_api_with_client;

use crate::xai_config::XaiConfig;
use crate::xai_error::XaiFailedResponseHandler;

use super::xai_video_options::XaiVideoMode;
use super::xai_video_options::XaiVideoProviderOptions;
use super::xai_video_options::extract_xai_video_options;
use super::xai_video_options::resolve_video_mode;

/// Default polling interval between status checks.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(5000);
/// Default overall polling timeout.
const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_millis(600_000);

/// Response from the video create endpoints. Mirrors
/// `xaiCreateVideoResponseSchema` — minimal subset of fields the impl reads.
#[derive(Debug, Deserialize)]
pub struct XaiCreateVideoResponse {
    pub request_id: Option<String>,
}

/// Response from `GET /videos/{request_id}`. Mirrors
/// `xaiVideoStatusResponseSchema` — minimal subset of fields the impl reads.
#[derive(Debug, Deserialize)]
pub struct XaiVideoStatusResponse {
    pub status: Option<String>,
    pub video: Option<XaiVideoStatusVideo>,
}

/// The `video` object on a status response.
#[derive(Debug, Deserialize)]
pub struct XaiVideoStatusVideo {
    pub url: Option<String>,
    pub respect_moderation: Option<bool>,
}

/// Map an SDK `WIDTHxHEIGHT` dimension string to an xAI resolution tier.
/// Mirrors `RESOLUTION_MAP` in `xai-video-model.ts`.
fn map_sdk_resolution(dimensions: &str) -> Option<&'static str> {
    match dimensions {
        "1280x720" => Some("720p"),
        "854x480" | "640x480" => Some("480p"),
        _ => None,
    }
}

/// The prepared create request: endpoint path and JSON body.
pub(crate) struct VideoRequestPlan {
    pub endpoint: &'static str,
    pub body: Value,
}

/// Build the create-request body + endpoint for a video call. Pure — mirrors
/// the body construction in `XaiVideoModel.doGenerate` (TS). The
/// `VideoModelV4` result type carries no warnings channel, so unsupported
/// inputs (`n > 1`, unrecognized SDK resolutions, edit-mode duration, …) are
/// silently omitted from the body where the TS impl warns and omits.
pub(crate) fn plan_video_request(
    model_id: &str,
    options: &VideoModelV4CallOptions,
    xai_options: &XaiVideoProviderOptions,
    extras: BTreeMap<String, Value>,
) -> Result<VideoRequestPlan, AISdkError> {
    let mode = resolve_video_mode(xai_options)?;
    let is_edit = mode == Some(XaiVideoMode::EditVideo);
    let is_extension = mode == Some(XaiVideoMode::ExtendVideo);
    let has_reference_images = mode == Some(XaiVideoMode::ReferenceToVideo);

    let mut body = json!({
        "model": model_id,
        "prompt": options.prompt,
    });

    let allow_duration = !is_edit;
    let allow_resolution = !is_edit && !is_extension;

    if allow_duration && let Some(ref duration) = options.duration {
        body["duration"] = json!(duration.seconds());
    }

    if allow_resolution {
        if let Some(resolution) = xai_options.resolution {
            body["resolution"] = json!(resolution.as_str());
        } else if let Some(ref size) = options.size {
            let (w, h) = size.dimensions();
            if let Some(mapped) = map_sdk_resolution(&format!("{w}x{h}")) {
                body["resolution"] = json!(mapped);
            }
        }
    }

    // Edit / extension: pass the source video URL (nested object).
    if (is_edit || is_extension)
        && let Some(ref video_url) = xai_options.video_url
    {
        body["video"] = json!({ "url": video_url });
    }

    // Start image (image-to-video input) as a data URI in the nested xAI
    // request image object.
    if let Some(ref image_bytes) = options.image {
        let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
        let mime = options.image_content_type.as_deref().unwrap_or("image/png");
        body["image"] = json!({ "url": format!("data:{mime};base64,{b64}") });
    }

    // Reference images for R2V generation.
    if has_reference_images && let Some(ref urls) = xai_options.reference_image_urls {
        let refs: Vec<Value> = urls.iter().map(|url| json!({ "url": url })).collect();
        body["reference_images"] = json!(refs);
    }

    // Pass through unrecognized provider-option keys (the upstream schema is
    // a loose object); extras win over typed writes.
    if !extras.is_empty() {
        let overlay = Value::Object(extras.into_iter().collect());
        body = merge_json_value(&body, &overlay);
    }

    let endpoint = if is_edit {
        "/videos/edits"
    } else if is_extension {
        "/videos/extensions"
    } else {
        "/videos/generations"
    };

    Ok(VideoRequestPlan { endpoint, body })
}

/// xAI video generation model (`grok-imagine-video` family).
///
/// Mirrors `XaiVideoModel` from `@ai-sdk/xai`: an async create → poll flow.
/// `POST /videos/generations` (or `/videos/edits` / `/videos/extensions`
/// depending on the resolved mode) returns a `request_id`, which is polled
/// via `GET /videos/{request_id}` until `done` / `failed` / `expired`, with a
/// bounded timeout (`pollTimeoutMs`, default 600s) and configurable interval
/// (`pollIntervalMs`, default 5s).
pub struct XaiVideoModel {
    model_id: String,
    config: Arc<XaiConfig>,
}

impl XaiVideoModel {
    /// Create a new xAI video model instance.
    pub fn new(model_id: impl Into<String>, config: Arc<XaiConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl VideoModelV4 for XaiVideoModel {
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
        let mut xai_options = extract_xai_video_options(&options.provider_options)?;
        let extras = std::mem::take(&mut xai_options.extra);

        let poll_interval = xai_options
            .poll_interval_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_POLL_INTERVAL);
        let poll_timeout = xai_options
            .poll_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_POLL_TIMEOUT);

        let plan = plan_video_request(&self.model_id, &options, &xai_options, extras)?;

        let mut headers = self.config.get_headers();
        if let Some(ref extra) = options.headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }

        // Step 1: create the generation / edit / extension request.
        let url = self.config.url(plan.endpoint);
        let create_response: XaiCreateVideoResponse = post_json_to_api_with_client(
            &url,
            Some(headers.clone()),
            &plan.body,
            JsonResponseHandler::new(),
            XaiFailedResponseHandler,
            options.abort_signal.clone(),
            self.config.client.clone(),
        )
        .await?;

        let request_id = create_response
            .request_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AISdkError::new("No request_id returned from xAI API."))?;

        // Step 2: poll for completion, bounded by `poll_timeout`.
        let poll_url = self.config.url(&format!("/videos/{request_id}"));
        let start = std::time::Instant::now();

        loop {
            delay(poll_interval).await;

            if start.elapsed() > poll_timeout {
                return Err(AISdkError::new(format!(
                    "Video generation timed out after {}ms",
                    poll_timeout.as_millis()
                )));
            }

            let status: XaiVideoStatusResponse = get_from_api_with_client(
                &poll_url,
                Some(headers.clone()),
                JsonResponseHandler::new(),
                XaiFailedResponseHandler,
                options.abort_signal.clone(),
                self.config.client.clone(),
            )
            .await?;

            let video_url = status.video.as_ref().and_then(|v| v.url.as_deref());
            let is_done = status.status.as_deref() == Some("done")
                || (status.status.is_none() && video_url.is_some());

            if is_done {
                if status.video.as_ref().and_then(|v| v.respect_moderation) == Some(false) {
                    return Err(AISdkError::new(
                        "Video generation was blocked due to a content policy violation.",
                    ));
                }
                let Some(video_url) = video_url else {
                    return Err(AISdkError::new(
                        "Video generation completed but no video URL was returned.",
                    ));
                };
                return Ok(VideoModelV4Result {
                    videos: vec![GeneratedVideo::url(video_url).with_content_type("video/mp4")],
                });
            }

            match status.status.as_deref() {
                Some("expired") => {
                    return Err(AISdkError::new("Video generation request expired."));
                }
                Some("failed") => {
                    return Err(AISdkError::new("Video generation failed."));
                }
                // `pending` (or anything else) → continue polling.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "xai_video_model.test.rs"]
mod tests;
