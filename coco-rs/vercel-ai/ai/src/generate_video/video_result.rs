//! Video generation result types.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::Warning;

/// Result of a `generate_video` call.
#[derive(Debug)]
#[must_use]
pub struct GenerateVideoResult {
    /// The generated videos.
    pub videos: Vec<GeneratedVideo>,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
    /// The model ID used.
    pub model_id: String,
}

impl GenerateVideoResult {
    /// Create a new video generation result.
    pub fn new(videos: Vec<GeneratedVideo>, model_id: impl Into<String>) -> Self {
        Self {
            videos,
            warnings: Vec::new(),
            model_id: model_id.into(),
        }
    }

    /// Get the first video (convenience method).
    pub fn video(&self) -> Option<&GeneratedVideo> {
        self.videos.first()
    }

    /// Add warnings.
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }
}

/// A generated video.
#[derive(Debug, Clone)]
pub struct GeneratedVideo {
    /// The video data (either URL, base64, or binary).
    pub data: VideoData,
    /// The MIME type of the video (e.g., "video/mp4").
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

    /// Create a video from binary data.
    pub fn binary(data: Vec<u8>) -> Self {
        Self {
            data: VideoData::Binary(data),
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

    /// Get the binary data if this is a binary video.
    pub fn as_binary(&self) -> Option<&[u8]> {
        match &self.data {
            VideoData::Binary(data) => Some(data),
            _ => None,
        }
    }

    /// Get the data as base64 string (converts binary if needed).
    pub fn to_base64(&self) -> String {
        match &self.data {
            VideoData::Base64(data) => data.clone(),
            VideoData::Binary(data) => BASE64.encode(data),
            VideoData::Url(_) => String::new(),
        }
    }

    /// Get the data as binary (decodes base64 if needed).
    pub fn to_binary(&self) -> Vec<u8> {
        match &self.data {
            VideoData::Binary(data) => data.clone(),
            VideoData::Base64(data) => BASE64.decode(data).unwrap_or_default(),
            VideoData::Url(_) => Vec::new(),
        }
    }

    /// Get the file extension based on the content type.
    pub fn extension(&self) -> &str {
        match self.content_type.as_deref() {
            Some("video/mp4") => "mp4",
            Some("video/webm") => "webm",
            Some("video/quicktime") => "mov",
            Some("video/x-msvideo") => "avi",
            Some("video/x-matroska") => "mkv",
            _ => "bin",
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
    /// Binary video data.
    Binary(Vec<u8>),
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

    /// Parse from a string like "1920x1080".
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

impl std::fmt::Display for VideoSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.dimensions();
        write!(f, "{w}x{h}")
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
