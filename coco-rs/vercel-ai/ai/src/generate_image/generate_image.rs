//! Generate images from text prompts.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::try_join_all;
use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::ImageModelV4;
use vercel_ai_provider::ImageModelV4CallOptions;
use vercel_ai_provider::ImageSize as ProviderImageSize;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Warning;

use crate::error::AIError;
use crate::logger::LogWarningsOptions;
use crate::logger::log_warnings;
use crate::provider::get_default_provider;
use crate::types::ImageModelResponseMetadata;
use crate::types::ProviderOptions;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;

use super::image_result::GenerateImageResult;
use super::image_result::GeneratedImage;
use super::image_result::ImageSize;

/// A reference to an image model.
#[derive(Clone)]
pub enum ImageModel {
    /// A string model ID that will be resolved via the default provider.
    String(String),
    /// A pre-resolved image model.
    V4(Arc<dyn ImageModelV4>),
}

impl Default for ImageModel {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl ImageModel {
    /// Create from a string ID.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self::String(id.into())
    }

    /// Create from a V4 model.
    pub fn from_v4(model: Arc<dyn ImageModelV4>) -> Self {
        Self::V4(model)
    }

    /// Check if this is a string ID.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }
}

impl From<String> for ImageModel {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for ImageModel {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

impl From<Arc<dyn ImageModelV4>> for ImageModel {
    fn from(model: Arc<dyn ImageModelV4>) -> Self {
        Self::V4(model)
    }
}

/// Prompt for image generation.
#[derive(Debug, Clone)]
pub enum ImagePrompt {
    /// A text prompt.
    Text(String),
    /// A prompt with reference images.
    WithImages {
        /// The text prompt.
        text: String,
        /// Reference images (URLs or base64 data).
        images: Vec<String>,
        /// Optional mask image (URL or base64 data) for inpainting.
        mask: Option<String>,
    },
}

impl Default for ImagePrompt {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<String> for ImagePrompt {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for ImagePrompt {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

/// Options for `generate_image`.
#[derive(Default)]
pub struct GenerateImageOptions {
    /// The image model to use.
    pub model: ImageModel,
    /// The prompt for image generation.
    pub prompt: ImagePrompt,
    /// Number of images to generate.
    pub n: Option<usize>,
    /// Maximum images per API call.
    pub max_images_per_call: Option<usize>,
    /// Size of the images.
    pub size: Option<ImageSize>,
    /// Aspect ratio of the images (e.g., "16:9", "1:1").
    pub aspect_ratio: Option<String>,
    /// Seed for deterministic generation.
    pub seed: Option<i64>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Maximum retries (default: 2).
    pub max_retries: Option<u32>,
    /// Additional HTTP headers.
    pub headers: Option<HashMap<String, String>>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
}

impl GenerateImageOptions {
    /// Create new options with a model and prompt.
    pub fn new(model: impl Into<ImageModel>, prompt: impl Into<ImagePrompt>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    /// Set the number of images.
    pub fn with_n(mut self, n: usize) -> Self {
        self.n = Some(n);
        self
    }

    /// Set the maximum images per API call.
    pub fn with_max_images_per_call(mut self, max: usize) -> Self {
        self.max_images_per_call = Some(max);
        self
    }

    /// Set the size.
    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the aspect ratio.
    pub fn with_aspect_ratio(mut self, aspect_ratio: impl Into<String>) -> Self {
        self.aspect_ratio = Some(aspect_ratio.into());
        self
    }

    /// Set the seed.
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set provider-specific options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the maximum retries.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = Some(retries);
        self
    }

    /// Set additional HTTP headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }
}

/// Resolve an image model reference to an actual model instance.
fn resolve_image_model(model: ImageModel) -> Result<Arc<dyn ImageModelV4>, AIError> {
    match model {
        ImageModel::V4(m) => Ok(m),
        ImageModel::String(id) => {
            let provider = get_default_provider().ok_or_else(|| {
                AIError::InvalidArgument(
                    "No default provider set. Call set_default_provider() first or use an ImageModel::V4 variant.".to_string(),
                )
            })?;
            provider
                .image_model(&id)
                .map_err(|e| AIError::ProviderError(AISdkError::new(e.to_string())))
        }
    }
}

/// Generate images from a text prompt.
///
/// This function generates images using an image generation model.
/// When `n` exceeds `max_images_per_call`, multiple API calls are made in
/// parallel and the results are merged.
///
/// # Arguments
///
/// * `options` - The generation options including model, prompt, and settings.
///
/// # Returns
///
/// A `GenerateImageResult` containing the generated images.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{generate_image, GenerateImageOptions, ImageSize};
///
/// let result = generate_image(GenerateImageOptions {
///     model: "dall-e-3".into(),
///     prompt: "A serene landscape with mountains".into(),
///     size: Some(ImageSize::S1024x1024),
///     ..Default::default()
/// }).await?;
///
/// // Get the first image
/// if let Some(image) = result.image() {
///     println!("Image URL: {:?}", image.as_url());
/// }
/// ```
pub async fn generate_image(options: GenerateImageOptions) -> Result<GenerateImageResult, AIError> {
    let model = resolve_image_model(options.model)?;
    let model_id = model.model_id().to_string();
    let provider_id = model.provider().to_string();

    // Get the number of images to generate
    let n = options.n.unwrap_or(1);

    // Determine how many images per call
    let max_per_call = options
        .max_images_per_call
        .unwrap_or_else(|| model.max_images_per_call());

    // Calculate the number of calls and images per call
    let call_count = n.div_ceil(max_per_call);

    // Extract prompt text
    let prompt_text = match &options.prompt {
        ImagePrompt::Text(text) => text.clone(),
        ImagePrompt::WithImages { text, .. } => text.clone(),
    };

    // Set up retry configuration
    let retry_config = options
        .max_retries
        .map(|max_retries| RetryConfig::new().with_max_retries(max_retries))
        .unwrap_or_default();

    // Convert size
    let provider_size = options.size.map(|size| match size {
        ImageSize::S256x256 => ProviderImageSize::S256x256,
        ImageSize::S512x512 => ProviderImageSize::S512x512,
        ImageSize::S1024x1024 => ProviderImageSize::S1024x1024,
        ImageSize::S1792x1024 => ProviderImageSize::S1792x1024,
        ImageSize::S1024x1792 => ProviderImageSize::S1024x1792,
        ImageSize::Custom { width, height } => ProviderImageSize::Custom { width, height },
    });

    // Build futures for parallel execution
    let futures: Vec<_> = (0..call_count)
        .map(|i| {
            let remaining = n - i * max_per_call;
            let count = remaining.min(max_per_call);
            let model = model.clone();
            let prompt_text = prompt_text.clone();
            let retry_config = retry_config.clone();
            let abort_signal = options.abort_signal.clone();
            let provider_options = options.provider_options.clone();
            let headers = options.headers.clone();
            let aspect_ratio = options.aspect_ratio.clone();
            let seed = options.seed;

            async move {
                let mut call_options = ImageModelV4CallOptions::new(&prompt_text);
                call_options = call_options.with_n(count);

                if let Some(size) = provider_size {
                    call_options = call_options.with_size(size);
                }

                if let Some(ref ar) = aspect_ratio {
                    call_options = call_options.with_aspect_ratio(ar.clone());
                }

                if let Some(seed) = seed {
                    call_options = call_options.with_seed(seed);
                }

                if let Some(ref signal) = abort_signal {
                    call_options = call_options.with_abort_signal(signal.clone());
                }

                if let Some(ref opts) = provider_options {
                    call_options = call_options.with_provider_options(opts.clone());
                }

                if let Some(ref h) = headers {
                    call_options.headers = Some(h.clone());
                }

                with_retry(retry_config, abort_signal, || {
                    let model = model.clone();
                    let opts = call_options.clone();
                    async move { model.do_generate(opts).await.map_err(AIError::from) }
                })
                .await
            }
        })
        .collect();

    // Execute all calls in parallel
    let results = try_join_all(futures).await?;

    // Merge results
    let mut all_images: Vec<GeneratedImage> = Vec::new();
    let mut all_warnings: Vec<Warning> = Vec::new();
    let mut all_responses: Vec<ImageModelResponseMetadata> = Vec::new();
    let mut last_provider_metadata: Option<ProviderMetadata> = None;
    let mut last_usage = None;

    for result in results {
        // Convert images
        for img in result.images {
            let generated = match img.data {
                vercel_ai_provider::ImageData::Url(url) => GeneratedImage::url(url),
                vercel_ai_provider::ImageData::Base64(data) => GeneratedImage::base64(data),
            };
            let generated = if let Some(mt) = img.media_type {
                generated.with_media_type(mt)
            } else {
                generated
            };
            all_images.push(generated);
        }

        // Collect warnings
        all_warnings.extend(result.warnings);

        // Build response metadata from provider response
        let mut response_meta = ImageModelResponseMetadata::new()
            .with_model_id(result.response.model_id.as_deref().unwrap_or(&model_id));
        if let Some(ts) = result.response.timestamp
            && let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts)
        {
            response_meta = response_meta.with_timestamp(parsed.with_timezone(&chrono::Utc));
        }
        if let Some(headers) = result.response.headers {
            response_meta = response_meta.with_headers(headers);
        }
        all_responses.push(response_meta);

        // Keep the last provider metadata
        if result.provider_metadata.is_some() {
            last_provider_metadata = result.provider_metadata;
        }

        // Keep the last usage
        if result.usage.is_some() {
            last_usage = result.usage;
        }
    }

    // Log warnings
    log_warnings(&LogWarningsOptions::new(
        all_warnings.clone(),
        &provider_id,
        &model_id,
    ));

    // Check if images were generated
    if all_images.is_empty() {
        return Err(AIError::NoImageGenerated);
    }

    // Build the result
    let mut image_result =
        GenerateImageResult::new(all_images, model_id).with_responses(all_responses);

    if !all_warnings.is_empty() {
        image_result = image_result.with_warnings(all_warnings);
    }

    if let Some(usage) = last_usage {
        image_result = image_result.with_usage(usage);
    }

    if let Some(metadata) = last_provider_metadata {
        image_result = image_result.with_provider_metadata(metadata);
    }

    Ok(image_result)
}

#[cfg(test)]
#[path = "generate_image.test.rs"]
mod tests;
