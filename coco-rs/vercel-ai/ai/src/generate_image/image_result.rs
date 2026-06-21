//! Image generation result types.

use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::ImageModelV4Usage;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::Warning;

use crate::types::ImageModelResponseMetadata;

/// Result of a `generate_image` call.
#[derive(Debug)]
#[must_use]
pub struct GenerateImageResult {
    /// The generated images.
    pub images: Vec<GeneratedImage>,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// Token usage (if available).
    pub usage: Option<ImageModelV4Usage>,
    /// The model ID used.
    pub model_id: String,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Response metadata from each API call.
    pub responses: Vec<ImageModelResponseMetadata>,
}

impl GenerateImageResult {
    /// Create a new image generation result.
    pub fn new(images: Vec<GeneratedImage>, model_id: impl Into<String>) -> Self {
        Self {
            images,
            warnings: Vec::new(),
            usage: None,
            model_id: model_id.into(),
            provider_metadata: None,
            responses: Vec::new(),
        }
    }

    /// Get the first image (convenience method).
    pub fn image(&self) -> Option<&GeneratedImage> {
        self.images.first()
    }

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Set usage.
    pub fn with_usage(mut self, usage: ImageModelV4Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Set response metadata.
    pub fn with_responses(mut self, responses: Vec<ImageModelResponseMetadata>) -> Self {
        self.responses = responses;
        self
    }
}

/// A generated image.
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    /// The image data (either URL or base64).
    pub data: ImageData,
    /// The MIME type of the image (e.g., "image/png", "image/jpeg").
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

    /// Create an image from raw bytes.
    pub fn bytes(data: Vec<u8>) -> Self {
        Self {
            data: ImageData::Bytes(data),
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

    /// Get the raw bytes if available.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.data {
            ImageData::Bytes(data) => Some(data),
            _ => None,
        }
    }

    /// Convert to bytes, potentially decoding base64.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        match &self.data {
            ImageData::Bytes(data) => Some(data.clone()),
            ImageData::Base64(data) => {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.decode(data).ok()
            }
            ImageData::Url(_) => None,
        }
    }

    /// Get the file extension based on the media type.
    pub fn extension(&self) -> &str {
        match self.media_type.as_deref() {
            Some("image/png") => "png",
            Some("image/jpeg") | Some("image/jpg") => "jpg",
            Some("image/gif") => "gif",
            Some("image/webp") => "webp",
            Some("image/svg+xml") => "svg",
            _ => "bin",
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
    /// Raw image bytes.
    Bytes(Vec<u8>),
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

    /// Parse from a string like "1024x1024".
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() != 2 {
            return None;
        }
        let width: u32 = parts[0].parse().ok()?;
        let height: u32 = parts[1].parse().ok()?;
        Some(Self::Custom { width, height })
    }
}

impl std::fmt::Display for ImageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.dimensions();
        write!(f, "{w}x{h}")
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
