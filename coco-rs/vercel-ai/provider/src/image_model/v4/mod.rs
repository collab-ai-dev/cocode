//! Image model trait and related types (V4).
//!
//! This module defines the `ImageModelV4` trait for implementing image generation models
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use crate::errors::AISdkError;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::Warning;

/// An image file that can be used for image editing or variation generation.
#[derive(Debug, Clone)]
pub enum ImageModelV4File {
    /// A file with inline data and media type.
    File {
        media_type: String,
        data: ImageFileData,
        provider_options: Option<ProviderOptions>,
    },
    /// A file referenced by URL.
    Url {
        url: String,
        provider_options: Option<ProviderOptions>,
    },
}

/// Image file data — either base64-encoded or raw binary.
#[derive(Debug, Clone)]
pub enum ImageFileData {
    Base64(String),
    Binary(Vec<u8>),
}

/// The image model trait (V4).
///
/// This trait defines the interface for image generation models following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait ImageModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Get the maximum images per call.
    fn max_images_per_call(&self) -> usize {
        1
    }

    /// Generate images.
    async fn do_generate(
        &self,
        options: ImageModelV4CallOptions,
    ) -> Result<ImageModelV4GenerateResult, AISdkError>;
}

/// Options for an image model call.
#[derive(Debug, Clone, Default)]
pub struct ImageModelV4CallOptions {
    /// The prompt for image generation.
    pub prompt: String,
    /// The number of images to generate.
    pub n: Option<usize>,
    /// The size of the generated images.
    pub size: Option<ImageSize>,
    /// The quality of the generated images.
    pub quality: Option<ImageQuality>,
    /// The style of the generated images.
    pub style: Option<ImageStyle>,
    /// The response format.
    pub response_format: Option<ImageResponseFormat>,
    /// Aspect ratio of the generated images (e.g., "16:9", "1:1").
    pub aspect_ratio: Option<String>,
    /// Seed for deterministic generation.
    pub seed: Option<i64>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Input image files for editing / variations.
    pub files: Option<Vec<ImageModelV4File>>,
    /// Mask image indicating where to edit.
    pub mask: Option<ImageModelV4File>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
}

impl ImageModelV4CallOptions {
    /// Create new call options with a prompt.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    /// Set the number of images.
    pub fn with_n(mut self, n: usize) -> Self {
        self.n = Some(n);
        self
    }

    /// Set the size.
    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the quality.
    pub fn with_quality(mut self, quality: ImageQuality) -> Self {
        self.quality = Some(quality);
        self
    }

    /// Set the style.
    pub fn with_style(mut self, style: ImageStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Set the response format.
    pub fn with_response_format(mut self, format: ImageResponseFormat) -> Self {
        self.response_format = Some(format);
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

    /// Set provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set input image files for editing.
    pub fn with_files(mut self, files: Vec<ImageModelV4File>) -> Self {
        self.files = Some(files);
        self
    }

    /// Set a mask image for editing.
    pub fn with_mask(mut self, mask: ImageModelV4File) -> Self {
        self.mask = Some(mask);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }
}

/// Image size options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageSize {
    /// 256x256 pixels.
    #[serde(rename = "256x256")]
    S256x256,
    /// 512x512 pixels.
    #[serde(rename = "512x512")]
    S512x512,
    /// 1024x1024 pixels.
    #[serde(rename = "1024x1024")]
    S1024x1024,
    /// 1792x1024 pixels (landscape).
    #[serde(rename = "1792x1024")]
    S1792x1024,
    /// 1024x1792 pixels (portrait).
    #[serde(rename = "1024x1792")]
    S1024x1792,
    /// Custom size.
    Custom { width: u32, height: u32 },
}

impl ImageSize {
    /// Get the dimensions as (width, height).
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::S256x256 => (256, 256),
            Self::S512x512 => (512, 512),
            Self::S1024x1024 => (1024, 1024),
            Self::S1792x1024 => (1792, 1024),
            Self::S1024x1792 => (1024, 1792),
            Self::Custom { width, height } => (*width, *height),
        }
    }
}

/// Image quality options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageQuality {
    /// Standard quality.
    #[default]
    Standard,
    /// High definition quality.
    Hd,
}

/// Image style options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageStyle {
    /// Vivid style (default for DALL-E 3).
    #[default]
    Vivid,
    /// Natural style.
    Natural,
}

/// Image response format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageResponseFormat {
    /// URL to the image.
    #[default]
    Url,
    /// Base64-encoded image data.
    B64Json,
}

/// The result of an image generation call.
#[derive(Debug, Clone)]
pub struct ImageModelV4GenerateResult {
    /// The generated images (either URLs or base64 data).
    pub images: Vec<GeneratedImage>,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Response information.
    pub response: ImageModelV4Response,
    /// Token usage for image generation (if available).
    pub usage: Option<ImageModelV4Usage>,
}

impl ImageModelV4GenerateResult {
    /// Create a new image generation result.
    pub fn new(images: Vec<GeneratedImage>) -> Self {
        Self {
            images,
            warnings: Vec::new(),
            provider_metadata: None,
            response: ImageModelV4Response::default(),
            usage: None,
        }
    }

    /// Create from URLs.
    pub fn from_urls(urls: Vec<String>) -> Self {
        let images = urls.into_iter().map(GeneratedImage::url).collect();
        Self::new(images)
    }

    /// Create from base64 data.
    pub fn from_base64(data: Vec<String>) -> Self {
        let images = data.into_iter().map(GeneratedImage::base64).collect();
        Self::new(images)
    }

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Add provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Set the response.
    pub fn with_response(mut self, response: ImageModelV4Response) -> Self {
        self.response = response;
        self
    }

    /// Set usage.
    pub fn with_usage(mut self, usage: ImageModelV4Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

/// A generated image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    /// The image data (either URL or base64).
    pub data: ImageData,
    /// The MIME type of the image.
    pub media_type: Option<String>,
}

impl GeneratedImage {
    /// Create an image from a URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            data: ImageData::Url(url.into()),
            media_type: None,
        }
    }

    /// Create an image from base64 data.
    pub fn base64(data: impl Into<String>) -> Self {
        Self {
            data: ImageData::Base64(data.into()),
            media_type: None,
        }
    }

    /// Set the media type.
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    /// Get the URL if this is a URL image.
    pub fn as_url(&self) -> Option<&str> {
        match &self.data {
            ImageData::Url(url) => Some(url),
            _ => None,
        }
    }

    /// Get the base64 data if this is a base64 image.
    pub fn as_base64(&self) -> Option<&str> {
        match &self.data {
            ImageData::Base64(data) => Some(data),
            _ => None,
        }
    }
}

/// Image data container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageData {
    /// URL to the image.
    Url(String),
    /// Base64-encoded image data.
    Base64(String),
}

/// Response information for image generation.
#[derive(Debug, Clone, Default)]
pub struct ImageModelV4Response {
    /// The timestamp of the response.
    pub timestamp: Option<String>,
    /// The model ID used.
    pub model_id: Option<String>,
    /// Response headers.
    pub headers: Option<HashMap<String, String>>,
}

impl ImageModelV4Response {
    /// Create new response info.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Set the model ID.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Set the headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }
}

/// Token usage for image generation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageModelV4Usage {
    /// The number of tokens in the prompt.
    pub prompt_tokens: u64,
    /// The number of output tokens.
    pub output_tokens: u64,
    /// The total number of tokens.
    pub total_tokens: u64,
}

impl ImageModelV4Usage {
    /// Create new usage info.
    pub fn new(prompt_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            output_tokens: 0,
            total_tokens: prompt_tokens,
        }
    }
}

#[cfg(test)]
#[path = "image_model_v4.test.rs"]
mod tests;
