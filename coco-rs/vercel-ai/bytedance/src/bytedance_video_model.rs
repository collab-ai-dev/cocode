//! ByteDance Seedance video model implementation.
//!
//! Uses async polling: POST to create task, GET to poll status until succeeded/failed.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4CallOptions;
use vercel_ai_provider::VideoModelV4Result;
use vercel_ai_provider::video_model::v4::GeneratedVideo;

use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::combine_headers;
use vercel_ai_provider_utils::delay;
use vercel_ai_provider_utils::get_from_api_with_client;
use vercel_ai_provider_utils::post_json_to_api_with_client;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::bytedance_config::ByteDanceVideoModelConfig;
use crate::bytedance_error::ByteDanceFailedResponseHandler;
use crate::bytedance_video_options::ByteDanceVideoProviderOptions;

/// ByteDance Seedance video model.
///
/// Generates videos using the ByteDance ModelArk API with async task polling.
pub struct ByteDanceVideoModel {
    model_id: String,
    config: ByteDanceVideoModelConfig,
}

impl ByteDanceVideoModel {
    /// Create a new ByteDance video model.
    pub fn new(model_id: impl Into<String>, config: ByteDanceVideoModelConfig) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    fn poll_interval(&self) -> Duration {
        self.config.poll_interval.unwrap_or(Duration::from_secs(3))
    }

    fn poll_timeout(&self) -> Duration {
        self.config.poll_timeout.unwrap_or(Duration::from_secs(300))
    }
}

/// Map a `WIDTHxHEIGHT` dimension string to a resolution tier (480p/720p/1080p).
///
/// Returns the mapped resolution string, or the original string if not in the map.
pub fn map_resolution(dimensions: &str) -> &str {
    match dimensions {
        // 480p
        "864x496" | "496x864" | "752x560" | "560x752" | "640x640" | "992x432" | "432x992"
        | "864x480" | "480x864" | "736x544" | "544x736" | "960x416" | "416x960" | "832x480"
        | "480x832" | "624x624" => "480p",

        // 720p
        "1280x720" | "720x1280" | "1112x834" | "834x1112" | "960x960" | "1470x630" | "630x1470"
        | "1248x704" | "704x1248" | "1120x832" | "832x1120" | "1504x640" | "640x1504" => "720p",

        // 1080p
        "1920x1080" | "1080x1920" | "1664x1248" | "1248x1664" | "1440x1440" | "2206x946"
        | "946x2206" | "1920x1088" | "1088x1920" | "2176x928" | "928x2176" => "1080p",

        _ => dimensions,
    }
}

#[async_trait]
impl VideoModelV4 for ByteDanceVideoModel {
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
        let base_url = without_trailing_slash(&self.config.base_url);
        let url = format!("{base_url}/contents/generations/tasks");

        let headers = combine_headers(vec![Some((self.config.headers)()), options.headers.clone()]);

        // Parse provider options. `#[serde(flatten)] extra` captures
        // unknown keys for deep-merge below; typed-consumed keys never
        // appear in `extra`, replacing the old hand-maintained
        // `HANDLED_PROVIDER_OPTIONS` blacklist.
        let mut provider_opts: ByteDanceVideoProviderOptions = options
            .provider_options
            .as_ref()
            .and_then(|po| po.get("bytedance"))
            .map(|opts| {
                let value = serde_json::to_value(opts).unwrap_or_default();
                serde_json::from_value(value).unwrap_or_default()
            })
            .unwrap_or_default();
        let raw_opts: BTreeMap<String, serde_json::Value> =
            std::mem::take(&mut provider_opts.extra);

        // Determine poll settings (provider options override config)
        let poll_interval = provider_opts
            .poll_interval_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| self.poll_interval());
        let poll_timeout = provider_opts
            .poll_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| self.poll_timeout());

        // Build content array
        let mut content = Vec::new();

        // Text prompt
        content.push(json!({
            "type": "text",
            "text": options.prompt,
        }));

        // Reference image from options.image
        if let Some(ref image_bytes) = options.image {
            let base64_data = base64::engine::general_purpose::STANDARD.encode(image_bytes);
            let mime = options.image_content_type.as_deref().unwrap_or("image/png");
            let data_uri = format!("data:{mime};base64,{base64_data}");
            content.push(json!({
                "type": "image_url",
                "image_url": { "url": data_uri },
            }));
        }

        // Last frame image from provider options
        if let Some(ref last_frame) = provider_opts.last_frame_image {
            content.push(json!({
                "type": "image_url",
                "image_url": { "url": last_frame },
                "role": "last_frame",
            }));
        }

        // Reference images from provider options
        if let Some(ref ref_images) = provider_opts.reference_images {
            for img in ref_images {
                content.push(json!({
                    "type": "image_url",
                    "image_url": { "url": img },
                    "role": "reference_image",
                }));
            }
        }

        // Build request body
        let mut body = json!({
            "model": self.model_id,
            "content": content,
        });

        // Map video size to resolution
        if let Some(ref size) = options.size {
            let (w, h) = size.dimensions();
            let dim_str = format!("{w}x{h}");
            let resolution = map_resolution(&dim_str);
            body["resolution"] = json!(resolution);
        }

        // Map video duration
        if let Some(ref duration) = options.duration {
            body["duration"] = json!(duration.seconds());
        }

        // Apply typed provider options
        if let Some(watermark) = provider_opts.watermark {
            body["watermark"] = json!(watermark);
        }
        if let Some(generate_audio) = provider_opts.generate_audio {
            body["generate_audio"] = json!(generate_audio);
        }
        if let Some(camera_fixed) = provider_opts.camera_fixed {
            body["camera_fixed"] = json!(camera_fixed);
        }
        if let Some(return_last_frame) = provider_opts.return_last_frame {
            body["return_last_frame"] = json!(return_last_frame);
        }
        if let Some(ref service_tier) = provider_opts.service_tier {
            body["service_tier"] = json!(service_tier);
        }
        if let Some(draft) = provider_opts.draft {
            body["draft"] = json!(draft);
        }

        // Deep-merge extra_body onto the wire body. Callers own
        // wire-correct nesting; deep merge places nested overlays
        // at the right slot without clobbering sibling typed writes.
        if !raw_opts.is_empty() {
            let overlay = serde_json::Value::Object(raw_opts.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        // POST to create task
        let create_response: Value = post_json_to_api_with_client(
            &url,
            Some(headers.clone()),
            &body,
            JsonResponseHandler::new(),
            ByteDanceFailedResponseHandler,
            options.abort_signal.clone(),
            self.config.client.clone(),
        )
        .await?;

        // Extract task_id
        let task_id = create_response
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AISdkError::new("No task id in response"))?
            .to_string();

        // Poll for completion
        let poll_url = format!("{base_url}/contents/generations/tasks/{task_id}");
        let start = std::time::Instant::now();

        loop {
            let status: Value = get_from_api_with_client(
                &poll_url,
                Some(headers.clone()),
                JsonResponseHandler::new(),
                ByteDanceFailedResponseHandler,
                options.abort_signal.clone(),
                self.config.client.clone(),
            )
            .await?;

            let task_status = status.get("status").and_then(|s| s.as_str()).unwrap_or("");

            match task_status {
                "succeeded" => {
                    // Extract video URL from content.video_url
                    let video_url = status
                        .get("content")
                        .and_then(|c| c.get("video_url"))
                        .and_then(|u| u.as_str())
                        .ok_or_else(|| {
                            AISdkError::new("No video_url in succeeded task response")
                        })?;

                    let video = GeneratedVideo::url(video_url).with_content_type("video/mp4");

                    return Ok(VideoModelV4Result {
                        videos: vec![video],
                    });
                }
                "failed" => {
                    let message = status
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("Video generation failed");
                    return Err(AISdkError::new(message));
                }
                _ => {
                    // Still processing, continue polling
                }
            }

            if start.elapsed() > poll_timeout {
                return Err(AISdkError::new(format!(
                    "Video generation timed out after {} seconds",
                    poll_timeout.as_secs()
                )));
            }

            delay(poll_interval).await;
        }
    }
}

#[cfg(test)]
#[path = "bytedance_video_model.test.rs"]
mod tests;
