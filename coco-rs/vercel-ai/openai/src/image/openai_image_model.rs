use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
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
use vercel_ai_provider::ImageSize;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Warning;
use vercel_ai_provider::image_model::v4::ImageModelV4Response;
use vercel_ai_provider::image_model::v4::ImageModelV4Usage;
use vercel_ai_provider_utils::FormData;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client;

use crate::openai_config::OpenAIConfig;
use crate::openai_error::OpenAIFailedResponseHandler;

use super::openai_image_api::OpenAIImageResponse;
use super::openai_image_api::OpenAIImageTokenDetails;
use super::openai_image_options::extract_raw_image_options;
use super::openai_image_options::has_default_response_format;
use super::openai_image_options::model_max_images_per_call;

/// OpenAI Image model (DALL-E, gpt-image-1, etc.).
pub struct OpenAIImageModel {
    model_id: String,
    config: Arc<OpenAIConfig>,
}

impl OpenAIImageModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAIConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

/// Distribute token details evenly across images, with the remainder
/// assigned to the last image so that summing across all entries gives the
/// exact total.
fn distribute_token_details(
    details: Option<&OpenAIImageTokenDetails>,
    index: usize,
    total: usize,
) -> serde_json::Value {
    let Some(details) = details else {
        return json!({});
    };

    let mut result = serde_json::Map::new();

    if let Some(image_tokens) = details.image_tokens {
        let base = image_tokens / total as u64;
        let remainder = image_tokens - base * (total as u64 - 1);
        let value = if index == total - 1 { remainder } else { base };
        result.insert("imageTokens".into(), json!(value));
    }

    if let Some(text_tokens) = details.text_tokens {
        let base = text_tokens / total as u64;
        let remainder = text_tokens - base * (total as u64 - 1);
        let value = if index == total - 1 { remainder } else { base };
        result.insert("textTokens".into(), json!(value));
    }

    serde_json::Value::Object(result)
}

/// Build provider metadata with per-image details (revised_prompt, token
/// distribution, response-level fields).
fn build_image_provider_metadata(response: &OpenAIImageResponse) -> Option<ProviderMetadata> {
    let total = response.data.len();
    let token_details = response
        .usage
        .as_ref()
        .and_then(|u| u.input_tokens_details.as_ref());

    let images: Vec<serde_json::Value> = response
        .data
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let mut img = serde_json::Map::new();
            if let Some(ref rp) = item.revised_prompt {
                img.insert("revisedPrompt".into(), json!(rp));
            }
            if let Some(created) = response.created {
                img.insert("created".into(), json!(created));
            }
            if let Some(ref size) = response.size {
                img.insert("size".into(), json!(size));
            }
            if let Some(ref quality) = response.quality {
                img.insert("quality".into(), json!(quality));
            }
            if let Some(ref bg) = response.background {
                img.insert("background".into(), json!(bg));
            }
            if let Some(ref fmt) = response.output_format {
                img.insert("outputFormat".into(), json!(fmt));
            }
            // Merge distributed token details.
            if let serde_json::Value::Object(tokens) =
                distribute_token_details(token_details, idx, total)
            {
                for (k, v) in tokens {
                    img.insert(k, v);
                }
            }
            serde_json::Value::Object(img)
        })
        .collect();

    let mut meta = ProviderMetadata::default();
    meta.0.insert("openai".into(), json!({ "images": images }));
    Some(meta)
}

#[async_trait]
impl ImageModelV4 for OpenAIImageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn max_images_per_call(&self) -> usize {
        model_max_images_per_call(&self.model_id)
    }

    async fn do_generate(
        &self,
        options: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4GenerateResult, AISdkError> {
        let raw_opts = extract_raw_image_options(&options.provider_options);
        let mut warnings: Vec<Warning> = Vec::new();

        // Warn about unsupported features.
        if options.aspect_ratio.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "aspectRatio".into(),
                details: Some(
                    "This model does not support aspect ratio. Use `size` instead.".into(),
                ),
            });
        }
        if options.seed.is_some() {
            warnings.push(Warning::Unsupported {
                feature: "seed".into(),
                details: None,
            });
        }

        let size_value = options.size.as_ref().map(|s| match s {
            ImageSize::S256x256 => "256x256".to_string(),
            ImageSize::S512x512 => "512x512".to_string(),
            ImageSize::S1024x1024 => "1024x1024".to_string(),
            ImageSize::S1792x1024 => "1792x1024".to_string(),
            ImageSize::S1024x1792 => "1024x1792".to_string(),
            ImageSize::Custom { width, height } => format!("{width}x{height}"),
        });

        let mut headers = self.config.get_headers();
        if let Some(ref extra) = options.headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }

        let (response, edit_headers) = if options.files.is_some() {
            let (resp, hdrs) = self
                .do_edit(&options, &raw_opts, size_value, &headers)
                .await?;
            (resp, Some(hdrs))
        } else {
            // NOTE: post_json_to_api_with_client doesn't return response headers.
            let resp = self
                .do_generation(&options, &raw_opts, size_value, &headers)
                .await?;
            (resp, None)
        };

        let media_type = derive_media_type(response.output_format.as_deref());
        let provider_metadata = build_image_provider_metadata(&response);

        let images: Vec<GeneratedImage> = response
            .data
            .iter()
            .filter_map(|d| {
                let data = if let Some(ref b64) = d.b64_json {
                    ImageData::Base64(b64.clone())
                } else if let Some(ref url) = d.url {
                    ImageData::Url(url.clone())
                } else {
                    return None;
                };
                Some(GeneratedImage {
                    data,
                    media_type: media_type.clone(),
                })
            })
            .collect();

        let usage = response.usage.as_ref().map(|u| ImageModelV4Usage {
            prompt_tokens: u.input_tokens.unwrap_or(0),
            output_tokens: u.output_tokens.unwrap_or(0),
            total_tokens: u.total_tokens.unwrap_or(0),
        });

        let timestamp = Some(chrono::Utc::now().to_rfc3339());

        Ok(ImageModelV4GenerateResult {
            images,
            warnings,
            provider_metadata,
            response: ImageModelV4Response {
                timestamp,
                model_id: Some(self.model_id.clone()),
                headers: edit_headers,
            },
            usage,
        })
    }
}

/// Derive MIME type from the response `output_format` field.
fn derive_media_type(output_format: Option<&str>) -> Option<String> {
    output_format.and_then(|fmt| match fmt {
        "png" => Some("image/png".into()),
        "jpeg" | "jpg" => Some("image/jpeg".into()),
        "webp" => Some("image/webp".into()),
        _ => None,
    })
}

/// Convert an `ImageModelV4File` into raw bytes + MIME type for multipart upload.
fn file_to_bytes(file: &ImageModelV4File) -> Result<(Vec<u8>, String), AISdkError> {
    match file {
        ImageModelV4File::File {
            media_type, data, ..
        } => {
            let bytes = match data {
                ImageFileData::Base64(b64) => {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .unwrap_or_default()
                }
                ImageFileData::Binary(bytes) => bytes.clone(),
            };
            Ok((bytes, media_type.clone()))
        }
        ImageModelV4File::Url { url, .. } => Err(AISdkError::new(format!(
            "URL-based image files are not supported for image editing. \
             Please provide the image data directly. URL: {url}"
        ))),
    }
}

impl OpenAIImageModel {
    /// Execute an image edit (POST /images/edits with multipart form).
    async fn do_edit(
        &self,
        options: &ImageModelV4CallOptions,
        raw_opts: &Option<serde_json::Map<String, serde_json::Value>>,
        size_value: Option<String>,
        headers: &HashMap<String, String>,
    ) -> Result<(OpenAIImageResponse, HashMap<String, String>), AISdkError> {
        let url = self.config.url("/images/edits");

        let mut form = FormData::new()
            .text("model", self.model_id.clone())
            .text("prompt", options.prompt.clone());

        // Attach image files.
        if let Some(ref files) = options.files {
            for (idx, file) in files.iter().enumerate() {
                let (bytes, mime) = file_to_bytes(file)?;
                let ext = mime_to_ext(&mime);
                let filename = format!("image{idx}.{ext}");
                form = form.bytes_with_mime("image", bytes, &filename, &mime);
            }
        }

        // Attach mask.
        if let Some(ref mask) = options.mask {
            let (bytes, mime) = file_to_bytes(mask)?;
            let ext = mime_to_ext(&mime);
            form = form.bytes_with_mime("mask", bytes, &format!("mask.{ext}"), &mime);
        }

        if let Some(n) = options.n {
            form = form.text("n", n.to_string());
        }
        if let Some(ref size) = size_value {
            form = form.text("size", size.clone());
        }

        // Merge provider options as form fields.
        if let Some(opts) = raw_opts {
            for (k, v) in opts {
                match v {
                    serde_json::Value::String(s) => {
                        form = form.text(k.as_str(), s.clone());
                    }
                    serde_json::Value::Number(n) => {
                        form = form.text(k.as_str(), n.to_string());
                    }
                    serde_json::Value::Bool(b) => {
                        form = form.text(k.as_str(), b.to_string());
                    }
                    _ => {}
                }
            }
        }

        let client = self
            .config
            .client
            .as_ref()
            .map(|c| c.as_ref().clone())
            .unwrap_or_default();

        let mut request = client.post(&url);
        for (k, v) in headers {
            request = request.header(k, v);
        }

        let http_response = request.multipart(form.build()).send().await.map_err(|e| {
            AISdkError::new(format!("OpenAI image edit request failed: {e}")).with_cause(Box::new(
                APICallError::new(e.to_string(), &url).with_retryable(e.is_timeout()),
            ))
        })?;

        let status = http_response.status();

        // Capture response headers before consuming the body.
        let response_headers: HashMap<String, String> = http_response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        if !status.is_success() {
            let body = http_response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".to_string());

            let message = match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(json) => json
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or(&body)
                    .to_string(),
                Err(_) => body.clone(),
            };

            let is_retryable = status.as_u16() == 429 || status.as_u16() >= 500;

            return Err(
                AISdkError::new(format!("OpenAI API error ({status}): {message}")).with_cause(
                    Box::new(
                        APICallError::new(&message, &url)
                            .with_status(status.as_u16())
                            .with_response_body(&body)
                            .with_retryable(is_retryable),
                    ),
                ),
            );
        }

        let body_text = http_response
            .text()
            .await
            .map_err(|e| AISdkError::new(format!("Failed to read image edit response: {e}")))?;

        let parsed = serde_json::from_str::<OpenAIImageResponse>(&body_text)
            .map_err(|e| AISdkError::new(format!("Failed to parse image edit response: {e}")))?;

        Ok((parsed, response_headers))
    }

    /// Execute an image generation (POST /images/generations with JSON body).
    async fn do_generation(
        &self,
        options: &ImageModelV4CallOptions,
        raw_opts: &Option<serde_json::Map<String, serde_json::Value>>,
        size_value: Option<String>,
        headers: &HashMap<String, String>,
    ) -> Result<OpenAIImageResponse, AISdkError> {
        // Start with provider options as base, then override with explicit fields.
        let mut body = if let Some(opts) = raw_opts {
            serde_json::Value::Object(opts.clone())
        } else {
            json!({})
        };

        // Explicit fields override provider options.
        body["model"] = json!(self.model_id);
        body["prompt"] = json!(options.prompt);

        if !has_default_response_format(&self.model_id) {
            body["response_format"] = json!("b64_json");
        }

        if let Some(n) = options.n {
            body["n"] = json!(n);
        }
        if let Some(ref size) = size_value {
            body["size"] = json!(size);
        }

        let url = self.config.url("/images/generations");

        post_json_to_api_with_client(
            &url,
            Some(headers.clone()),
            &body,
            JsonResponseHandler::new(),
            OpenAIFailedResponseHandler,
            options.abort_signal.clone(),
            self.config.client.clone(),
        )
        .await
    }
}

/// Map a MIME type to a file extension for multipart filenames.
fn mime_to_ext(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

#[cfg(test)]
#[path = "openai_image_model.test.rs"]
mod tests;
