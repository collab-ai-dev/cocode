use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::APICallError;
use vercel_ai_provider::GeneratedImage;
use vercel_ai_provider::ImageData;
use vercel_ai_provider::ImageFileData;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::ImageModelV4CallOptions;
use vercel_ai_provider::ImageModelV4File;
use vercel_ai_provider::ImageModelV4GenerateResult;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Warning;
use vercel_ai_provider::image_model::v4::ImageModelV4Response;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client;

use crate::xai_config::XaiConfig;
use crate::xai_error::XaiFailedResponseHandler;

use super::xai_image_options::XaiImageProviderOptions;
use super::xai_image_options::extract_xai_image_options;

/// Response from the xAI image generation / edit endpoints. Mirrors
/// `xaiImageResponseSchema` — a minimal subset of fields the impl reads.
#[derive(Debug, Deserialize)]
pub struct XaiImageResponse {
    pub data: Vec<XaiImageData>,
    pub usage: Option<XaiImageUsage>,
}

/// A single generated image in the response.
#[derive(Debug, Deserialize)]
pub struct XaiImageData {
    pub url: Option<String>,
    pub b64_json: Option<String>,
    pub revised_prompt: Option<String>,
}

/// Usage block on the image response (cost only; no token counts).
#[derive(Debug, Deserialize)]
pub struct XaiImageUsage {
    pub cost_in_usd_ticks: Option<i64>,
}

/// The prepared request: endpoint path, JSON body, and warnings.
pub(crate) struct ImageRequestPlan {
    pub endpoint: &'static str,
    pub body: Value,
    pub warnings: Vec<Warning>,
}

/// Convert an input image file to the URL string xAI accepts (a plain URL or
/// a `data:` URI). Mirrors `convertImageModelFileToDataUri`.
fn file_to_image_url(file: &ImageModelV4File) -> String {
    match file {
        ImageModelV4File::Url { url, .. } => url.clone(),
        ImageModelV4File::File {
            media_type, data, ..
        } => {
            let b64 = match data {
                ImageFileData::Base64(b64) => b64.clone(),
                ImageFileData::Binary(bytes) => {
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                }
            };
            format!("data:{media_type};base64,{b64}")
        }
    }
}

/// Build the request body + endpoint + warnings for an image call. Pure —
/// mirrors the body construction in `XaiImageModel.doGenerate` (TS).
pub(crate) fn plan_image_request(
    model_id: &str,
    options: &ImageModelV4CallOptions,
    xai_options: &XaiImageProviderOptions,
) -> ImageRequestPlan {
    let mut warnings: Vec<Warning> = Vec::new();

    if options.size.is_some() {
        warnings.push(Warning::Unsupported {
            feature: "size".into(),
            details: Some(
                "This model does not support the `size` option. Use `aspectRatio` instead.".into(),
            ),
        });
    }
    if options.seed.is_some() {
        warnings.push(Warning::Unsupported {
            feature: "seed".into(),
            details: None,
        });
    }
    if options.mask.is_some() {
        warnings.push(Warning::Unsupported {
            feature: "mask".into(),
            details: None,
        });
    }

    let image_urls: Vec<String> = options
        .files
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(file_to_image_url)
        .collect();
    let has_files = !image_urls.is_empty();

    let endpoint = if has_files {
        "/images/edits"
    } else {
        "/images/generations"
    };

    let mut body = json!({
        "model": model_id,
        "prompt": options.prompt,
        "response_format": "b64_json",
    });
    if let Some(n) = options.n {
        body["n"] = json!(n);
    }

    if let Some(ref aspect_ratio) = options.aspect_ratio {
        body["aspect_ratio"] = json!(aspect_ratio);
    } else if let Some(ref aspect_ratio) = xai_options.aspect_ratio {
        body["aspect_ratio"] = json!(aspect_ratio);
    }

    if let Some(ref output_format) = xai_options.output_format {
        body["output_format"] = json!(output_format);
    }
    if let Some(sync_mode) = xai_options.sync_mode {
        body["sync_mode"] = json!(sync_mode);
    }
    if let Some(resolution) = xai_options.resolution {
        body["resolution"] = json!(resolution.as_str());
    }
    if let Some(quality) = xai_options.quality {
        body["quality"] = json!(quality.as_str());
    }
    if let Some(ref user) = xai_options.user {
        body["user"] = json!(user);
    }

    match image_urls.as_slice() {
        [] => {}
        [single] => {
            body["image"] = json!({ "url": single, "type": "image_url" });
        }
        many => {
            let images: Vec<Value> = many
                .iter()
                .map(|url| json!({ "url": url, "type": "image_url" }))
                .collect();
            body["images"] = json!(images);
        }
    }

    ImageRequestPlan {
        endpoint,
        body,
        warnings,
    }
}

/// xAI image generation model (`grok-imagine-image` family).
///
/// Mirrors `XaiImageModel` from `@ai-sdk/xai`: text-to-image via
/// `POST /images/generations`, image editing via `POST /images/edits` when
/// input files are provided. Always requests `b64_json`; if the API returns
/// URLs instead, the images are downloaded and surfaced as base64.
pub struct XaiImageModel {
    model_id: String,
    config: Arc<XaiConfig>,
}

impl XaiImageModel {
    /// Create a new xAI image model instance.
    pub fn new(model_id: impl Into<String>, config: Arc<XaiConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Download an image URL to raw bytes (used when the API responds with
    /// URLs instead of `b64_json`). Mirrors the private `downloadImage` (TS).
    async fn download_image(&self, url: &str) -> Result<Vec<u8>, AISdkError> {
        let client = self
            .config
            .client
            .as_ref()
            .map(|c| c.as_ref().clone())
            .unwrap_or_default();

        let response = client.get(url).send().await.map_err(|e| {
            AISdkError::new(format!("Failed to download image: {e}")).with_cause(Box::new(
                APICallError::new(e.to_string(), url).with_retryable(e.is_timeout()),
            ))
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(
                AISdkError::new(format!("Failed to download image (HTTP {status})")).with_cause(
                    Box::new(
                        APICallError::new(format!("HTTP {status}"), url)
                            .with_status(status.as_u16()),
                    ),
                ),
            );
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read downloaded image: {e}")))?;
        Ok(bytes.to_vec())
    }
}

/// Build the `providerMetadata.xai` payload: per-image `revisedPrompt` plus
/// the response-level `costInUsdTicks`.
fn build_provider_metadata(response: &XaiImageResponse) -> ProviderMetadata {
    let images: Vec<Value> = response
        .data
        .iter()
        .map(|item| {
            let mut img = serde_json::Map::new();
            if let Some(ref rp) = item.revised_prompt {
                img.insert("revisedPrompt".into(), json!(rp));
            }
            Value::Object(img)
        })
        .collect();

    let mut xai = serde_json::Map::new();
    xai.insert("images".into(), json!(images));
    if let Some(cost) = response.usage.as_ref().and_then(|u| u.cost_in_usd_ticks) {
        xai.insert("costInUsdTicks".into(), json!(cost));
    }

    let mut meta = ProviderMetadata::default();
    meta.0.insert("xai".into(), Value::Object(xai));
    meta
}

#[async_trait]
impl ImageModelV4 for XaiImageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn max_images_per_call(&self) -> usize {
        3
    }

    async fn do_generate(
        &self,
        options: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4GenerateResult, AISdkError> {
        let xai_options = extract_xai_image_options(&options.provider_options);
        let plan = plan_image_request(&self.model_id, &options, &xai_options);

        let url = self.config.url(plan.endpoint);
        let mut headers = self.config.get_headers();
        if let Some(ref extra) = options.headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }

        let response: XaiImageResponse = post_json_to_api_with_client(
            &url,
            Some(headers),
            &plan.body,
            JsonResponseHandler::new(),
            XaiFailedResponseHandler,
            options.abort_signal.clone(),
            self.config.client.clone(),
        )
        .await?;

        // Prefer `b64_json` when every entry carries it; otherwise download
        // each image URL and surface the bytes as base64 (TS returns raw
        // bytes here — base64 is the equivalent data-bearing form in Rust).
        let all_base64 = response.data.iter().all(|d| d.b64_json.is_some());
        let mut images: Vec<GeneratedImage> = Vec::with_capacity(response.data.len());
        for item in &response.data {
            let data = match (&item.b64_json, all_base64) {
                (Some(b64), true) => ImageData::Base64(b64.clone()),
                _ => {
                    let image_url = item.url.as_deref().ok_or_else(|| {
                        AISdkError::new("xAI image response entry has neither b64_json nor url")
                    })?;
                    let bytes = self.download_image(image_url).await?;
                    ImageData::Base64(base64::engine::general_purpose::STANDARD.encode(bytes))
                }
            };
            images.push(GeneratedImage {
                data,
                media_type: None,
            });
        }

        let provider_metadata = Some(build_provider_metadata(&response));

        Ok(ImageModelV4GenerateResult {
            images,
            warnings: plan.warnings,
            provider_metadata,
            response: ImageModelV4Response {
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
                model_id: Some(self.model_id.clone()),
                headers: None,
            },
            usage: None,
        })
    }
}

#[cfg(test)]
#[path = "xai_image_model.test.rs"]
mod tests;
