use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::GeneratedImage;
use vercel_ai_provider::ImageData;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::ImageModelV4CallOptions;
use vercel_ai_provider::ImageModelV4GenerateResult;
use vercel_ai_provider::ImageSize;
use vercel_ai_provider::Warning;
use vercel_ai_provider::image_model::v4::ImageModelV4Response;
use vercel_ai_provider::image_model::v4::ImageModelV4Usage;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::post_json_to_api_with_client_and_headers;

use crate::openai_compatible_config::OpenAICompatibleConfig;

use super::openai_compatible_image_api::OpenAICompatibleImageResponse;
use super::openai_compatible_image_options::extract_image_options;

/// OpenAI-compatible Image model.
pub struct OpenAICompatibleImageModel {
    model_id: String,
    config: Arc<OpenAICompatibleConfig>,
}

impl OpenAICompatibleImageModel {
    pub fn new(model_id: impl Into<String>, config: Arc<OpenAICompatibleConfig>) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }
}

#[async_trait]
impl ImageModelV4 for OpenAICompatibleImageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn max_images_per_call(&self) -> usize {
        10
    }

    async fn do_generate(
        &self,
        options: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4GenerateResult, AISdkError> {
        let mut warnings = Vec::new();
        let provider_name = self.config.provider_options_name();
        let (compat_opts, passthrough) =
            extract_image_options(&options.provider_options, provider_name);

        // Warn about unsupported options
        if options.aspect_ratio.is_some() {
            warnings.push(Warning::unsupported_with_details(
                "aspectRatio",
                "This model does not support aspect ratio. Use `size` instead.",
            ));
        }
        if options.seed.is_some() {
            warnings.push(Warning::unsupported("seed"));
        }

        let size_str: Option<String> = options
            .size
            .as_ref()
            .map(|s| match s {
                ImageSize::S256x256 => "256x256".into(),
                ImageSize::S512x512 => "512x512".into(),
                ImageSize::S1024x1024 => "1024x1024".into(),
                ImageSize::S1792x1024 => "1792x1024".into(),
                ImageSize::S1024x1792 => "1024x1792".into(),
                ImageSize::Custom { width, height } => format!("{width}x{height}"),
            })
            .or_else(|| compat_opts.size.clone());

        let mut body = json!({
            "model": self.model_id,
            "prompt": options.prompt,
            "response_format": "b64_json",
        });

        if let Some(n) = options.n {
            body["n"] = json!(n);
        }
        if let Some(ref size) = size_str {
            body["size"] = serde_json::Value::String(size.clone());
        }
        if let Some(ref quality) = compat_opts.quality {
            body["quality"] = serde_json::Value::String(quality.clone());
        }
        if let Some(ref style) = compat_opts.style {
            body["style"] = serde_json::Value::String(style.clone());
        }
        if let Some(ref user) = compat_opts.user {
            body["user"] = serde_json::Value::String(user.clone());
        }

        // Deep-merge extra_body onto the wire body. Callers own
        // wire-correct nesting; deep merge places nested overlays
        // at the right slot without clobbering sibling typed writes.
        if !passthrough.is_empty() {
            let overlay = serde_json::Value::Object(passthrough.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        // Apply request body transform
        let body = self.config.transform_body(body);

        let url = self.config.url("/images/generations");
        let headers = self.config.get_headers();

        let api_response =
            post_json_to_api_with_client_and_headers::<OpenAICompatibleImageResponse>(
                &url,
                Some(headers),
                &body,
                JsonResponseHandler::new(),
                self.config.error_handler.clone(),
                options.abort_signal,
                self.config.client.clone(),
            )
            .await?;

        let response = api_response.value;
        let response_headers = api_response.headers;

        let images: Vec<GeneratedImage> = response
            .data
            .into_iter()
            .filter_map(|d| {
                let data = if let Some(b64) = d.b64_json {
                    ImageData::Base64(b64)
                } else if let Some(url) = d.url {
                    ImageData::Url(url)
                } else {
                    return None;
                };
                Some(GeneratedImage {
                    data,
                    media_type: Some("image/png".into()),
                })
            })
            .collect();

        let usage = response.usage.map(|u| ImageModelV4Usage {
            prompt_tokens: u.input_tokens.unwrap_or(0),
            output_tokens: u.output_tokens.unwrap_or(0),
            total_tokens: u.total_tokens.unwrap_or(0),
        });

        let timestamp = response
            .created
            .and_then(|ts| chrono::DateTime::from_timestamp(ts as i64, 0))
            .map(|dt| dt.to_rfc3339());

        Ok(ImageModelV4GenerateResult {
            images,
            warnings,
            provider_metadata: None,
            response: ImageModelV4Response {
                timestamp,
                model_id: Some(self.model_id.clone()),
                headers: Some(response_headers),
            },
            usage,
        })
    }
}

#[cfg(test)]
#[path = "openai_compatible_image_model.test.rs"]
mod tests;
