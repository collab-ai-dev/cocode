//! Video model trait and related types (V4).
//!
//! This module defines the `VideoModelV4` trait for implementing video generation models
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use crate::errors::AISdkError;
use crate::shared::ProviderOptions;

/// The video model trait (V4).
///
/// This trait defines the interface for video generation models following the
/// Vercel AI SDK v4 specification.
#[async_trait]
pub trait VideoModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Generate video from text or image prompt.
    async fn do_generate_video(
        &self,
        options: VideoModelV4CallOptions,
    ) -> Result<VideoModelV4Result, AISdkError>;
}

/// Options for a video model call.
#[derive(Debug, Clone, Default)]
pub struct VideoModelV4CallOptions {
    /// The prompt for video generation.
    pub prompt: String,
    /// An optional image to use as a reference or starting point.
    pub image: Option<Vec<u8>>,
    /// The MIME type of the reference image.
    pub image_content_type: Option<String>,
    /// The size of the generated video.
    pub size: Option<VideoSize>,
    /// The duration of the generated video.
    pub duration: Option<VideoDuration>,
    /// The number of videos to generate.
    pub n: Option<usize>,
    /// The style of the video.
    pub style: Option<String>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
}

impl VideoModelV4CallOptions {
    /// Create new call options with a prompt.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    /// Set the reference image.
    pub fn with_image(mut self, image: Vec<u8>, content_type: impl Into<String>) -> Self {
        self.image = Some(image);
        self.image_content_type = Some(content_type.into());
        self
    }

    /// Set the video size.
    pub fn with_size(mut self, size: VideoSize) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the duration.
    pub fn with_duration(mut self, duration: VideoDuration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Set the number of videos.
    pub fn with_n(mut self, n: usize) -> Self {
        self.n = Some(n);
        self
    }

    /// Set the style.
    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }

    /// Set provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }
}

/// The result of a video generation call.
#[derive(Debug, Clone)]
pub struct VideoModelV4Result {
    /// The generated videos.
    pub videos: Vec<GeneratedVideo>,
}

impl VideoModelV4Result {
    /// Create a new video generation result.
    pub fn new(videos: Vec<GeneratedVideo>) -> Self {
        Self { videos }
    }

    /// Create from URLs.
    pub fn from_urls(urls: Vec<String>) -> Self {
        let videos = urls.into_iter().map(GeneratedVideo::url).collect();
        Self::new(videos)
    }
}

/// A generated video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedVideo {
    /// The video data (either URL or base64).
    pub data: VideoData,
    /// The MIME type of the video.
    pub content_type: Option<String>,
}

impl GeneratedVideo {
    /// Create a video from a URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            data: VideoData::Url(url.into()),
            content_type: None,
        }
    }

    /// Create a video from base64 data.
    pub fn base64(data: impl Into<String>) -> Self {
        Self {
            data: VideoData::Base64(data.into()),
            content_type: None,
        }
    }

    /// Set the content type.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Get the URL if this is a URL video.
    pub fn as_url(&self) -> Option<&str> {
        match &self.data {
            VideoData::Url(url) => Some(url),
            _ => None,
        }
    }

    /// Get the base64 data if this is a base64 video.
    pub fn as_base64(&self) -> Option<&str> {
        match &self.data {
            VideoData::Base64(data) => Some(data),
            _ => None,
        }
    }
}

/// Video data container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoData {
    /// URL to the video.
    Url(String),
    /// Base64-encoded video data.
    Base64(String),
}

/// Video size options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoSize {
    /// 1280x720 pixels (720p).
    #[serde(rename = "1280x720")]
    HD720p,
    /// 1920x1080 pixels (1080p).
    #[serde(rename = "1920x1080")]
    HD1080p,
    /// 3840x2160 pixels (4K).
    #[serde(rename = "3840x2160")]
    UHD4K,
    /// Square video.
    #[serde(rename = "1080x1080")]
    Square,
    /// Portrait video.
    #[serde(rename = "1080x1920")]
    Portrait,
    /// Custom size.
    Custom { width: u32, height: u32 },
}

impl VideoSize {
    /// Get the dimensions as (width, height).
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::HD720p => (1280, 720),
            Self::HD1080p => (1920, 1080),
            Self::UHD4K => (3840, 2160),
            Self::Square => (1080, 1080),
            Self::Portrait => (1080, 1920),
            Self::Custom { width, height } => (*width, *height),
        }
    }
}

/// Video duration options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoDuration {
    /// 5 seconds.
    #[serde(rename = "5s")]
    Seconds5,
    /// 10 seconds.
    #[serde(rename = "10s")]
    Seconds10,
    /// 15 seconds.
    #[serde(rename = "15s")]
    Seconds15,
    /// Custom duration in seconds.
    Custom(u32),
}

impl VideoDuration {
    /// Get the duration in seconds.
    pub fn seconds(&self) -> u32 {
        match self {
            Self::Seconds5 => 5,
            Self::Seconds10 => 10,
            Self::Seconds15 => 15,
            Self::Custom(secs) => *secs,
        }
    }
}

#[cfg(test)]
#[path = "video_model_v4.test.rs"]
mod tests;
