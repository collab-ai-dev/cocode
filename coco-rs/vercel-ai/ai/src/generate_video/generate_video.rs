//! Generate videos from text prompts.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tokio_util::sync::CancellationToken;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::VideoData;
use vercel_ai_provider::VideoDuration as ProviderVideoDuration;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4CallOptions;
use vercel_ai_provider::VideoSize as ProviderVideoSize;
use vercel_ai_provider::Warning;

use crate::error::AIError;
use crate::error::NoVideoGeneratedError;
use crate::error::VideoModelResponseMetadata;
use crate::provider::get_default_provider;
use crate::util::detect_media_type::VIDEO_MEDIA_TYPE_SIGNATURES;
use crate::util::detect_media_type::detect_media_type;
use crate::util::download::DownloadResult;
use crate::util::download::download;
use crate::util::retry::RetryConfig;
use crate::util::retry::with_retry;
use crate::util::split_data_url::is_data_url;
use crate::util::split_data_url::is_http_url;
use crate::util::split_data_url::split_data_url;

use super::video_result::GenerateVideoResult;
use super::video_result::GeneratedVideo;
use super::video_result::VideoDuration;
use super::video_result::VideoSize;

/// A reference to a video model.
#[derive(Clone)]
pub enum VideoModel {
    /// A string model ID that will be resolved via the default provider.
    String(String),
    /// A pre-resolved video model.
    V4(Arc<dyn VideoModelV4>),
}

impl Default for VideoModel {
    fn default() -> Self {
        Self::String(String::new())
    }
}

impl VideoModel {
    /// Create from a string ID.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self::String(id.into())
    }

    /// Create from a V4 model.
    pub fn from_v4(model: Arc<dyn VideoModelV4>) -> Self {
        Self::V4(model)
    }

    /// Check if this is a string ID.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }
}

impl From<String> for VideoModel {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for VideoModel {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

impl From<Arc<dyn VideoModelV4>> for VideoModel {
    fn from(model: Arc<dyn VideoModelV4>) -> Self {
        Self::V4(model)
    }
}

/// Prompt for video generation.
#[derive(Debug, Clone)]
pub enum VideoPrompt {
    /// A text prompt.
    Text(String),
    /// A prompt with a reference image.
    WithImage {
        /// The text prompt.
        text: String,
        /// Reference image (URL, data URL, or base64 data).
        image: String,
        /// MIME type of the reference image (auto-detected if not provided).
        image_content_type: Option<String>,
    },
}

impl Default for VideoPrompt {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<String> for VideoPrompt {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for VideoPrompt {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

/// Aspect ratio format (e.g., "16:9", "9:16", "1:1").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AspectRatio {
    /// Width ratio component.
    pub width: u32,
    /// Height ratio component.
    pub height: u32,
}

impl AspectRatio {
    /// Create a new aspect ratio.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Parse from string format "{width}:{height}".
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let width: u32 = parts[0].parse().ok()?;
        let height: u32 = parts[1].parse().ok()?;
        Some(Self { width, height })
    }

    /// Create a 16:9 aspect ratio.
    pub fn landscape() -> Self {
        Self::new(16, 9)
    }

    /// Create a 9:16 aspect ratio.
    pub fn portrait() -> Self {
        Self::new(9, 16)
    }

    /// Create a 1:1 aspect ratio.
    pub fn square() -> Self {
        Self::new(1, 1)
    }
}

impl std::fmt::Display for AspectRatio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.width, self.height)
    }
}

/// Resolution format (e.g., "1920x1080").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Resolution {
    /// Create a new resolution.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Parse from string format "{width}x{height}".
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() != 2 {
            return None;
        }
        let width: u32 = parts[0].parse().ok()?;
        let height: u32 = parts[1].parse().ok()?;
        Some(Self { width, height })
    }

    /// Create HD resolution (1280x720).
    pub fn hd() -> Self {
        Self::new(1280, 720)
    }

    /// Create Full HD resolution (1920x1080).
    pub fn full_hd() -> Self {
        Self::new(1920, 1080)
    }

    /// Create 4K resolution (3840x2160).
    pub fn uhd_4k() -> Self {
        Self::new(3840, 2160)
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// Download function type for fetching videos from URLs.
pub type DownloadFn = Box<
    dyn Fn(
            reqwest::Url,
            Option<CancellationToken>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<DownloadResult, crate::util::download::DownloadError>,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

/// Options for `generate_video`.
#[derive(Default)]
pub struct GenerateVideoOptions {
    /// The video model to use.
    pub model: VideoModel,
    /// The prompt for video generation.
    pub prompt: VideoPrompt,
    /// Number of videos to generate.
    pub n: Option<usize>,
    /// Maximum videos per API call (for batching).
    pub max_videos_per_call: Option<usize>,
    /// Aspect ratio of the videos (e.g., "16:9").
    pub aspect_ratio: Option<AspectRatio>,
    /// Resolution of the videos (e.g., "1920x1080").
    pub resolution: Option<Resolution>,
    /// Size of the videos (legacy, prefer aspect_ratio/resolution).
    pub size: Option<VideoSize>,
    /// Duration of the videos in seconds.
    pub duration: Option<VideoDuration>,
    /// Frames per second.
    pub fps: Option<u32>,
    /// Seed for deterministic generation.
    pub seed: Option<u64>,
    /// Style of the videos.
    pub style: Option<String>,
    /// Additional HTTP headers.
    pub headers: Option<HashMap<String, String>>,
    /// Provider-specific options.
    pub provider_options: Option<ProviderOptions>,
    /// Maximum retries (default: 2).
    pub max_retries: Option<u32>,
    /// Abort signal for cancellation.
    pub abort_signal: Option<CancellationToken>,
    /// Custom download function for URL results.
    pub download: Option<DownloadFn>,
}

impl GenerateVideoOptions {
    /// Create new options with a model and prompt.
    pub fn new(model: impl Into<VideoModel>, prompt: impl Into<VideoPrompt>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    /// Set the number of videos.
    pub fn with_n(mut self, n: usize) -> Self {
        self.n = Some(n);
        self
    }

    /// Set the max videos per call.
    pub fn with_max_videos_per_call(mut self, max: usize) -> Self {
        self.max_videos_per_call = Some(max);
        self
    }

    /// Set the aspect ratio.
    pub fn with_aspect_ratio(mut self, ratio: AspectRatio) -> Self {
        self.aspect_ratio = Some(ratio);
        self
    }

    /// Set the resolution.
    pub fn with_resolution(mut self, resolution: Resolution) -> Self {
        self.resolution = Some(resolution);
        self
    }

    /// Set the size.
    pub fn with_size(mut self, size: VideoSize) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the duration.
    pub fn with_duration(mut self, duration: VideoDuration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Set the FPS.
    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Set the seed.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set the style.
    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }

    /// Set the headers.
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set the provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Set the max retries.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = Some(retries);
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set a custom download function.
    pub fn with_download(mut self, download_fn: DownloadFn) -> Self {
        self.download = Some(download_fn);
        self
    }
}

/// Resolve a video model reference to an actual model instance.
fn resolve_video_model(model: VideoModel) -> Result<Arc<dyn VideoModelV4>, AIError> {
    match model {
        VideoModel::V4(m) => Ok(m),
        VideoModel::String(id) => {
            let provider = get_default_provider().ok_or_else(|| {
                AIError::InvalidArgument(
                    "No default provider set. Call set_default_provider() first or use a VideoModel::V4 variant.".to_string(),
                )
            })?;
            provider
                .video_model(&id)
                .map_err(|e| AIError::ProviderError(AISdkError::new(e.to_string())))
        }
    }
}

/// Convert local VideoSize to provider VideoSize.
fn to_provider_size(size: VideoSize) -> ProviderVideoSize {
    match size {
        VideoSize::HD720p => ProviderVideoSize::HD720p,
        VideoSize::HD1080p => ProviderVideoSize::HD1080p,
        VideoSize::UHD4K => ProviderVideoSize::UHD4K,
        VideoSize::Square => ProviderVideoSize::Square,
        VideoSize::Portrait => ProviderVideoSize::Portrait,
        VideoSize::Custom { width, height } => ProviderVideoSize::Custom { width, height },
    }
}

/// Convert local VideoDuration to provider VideoDuration.
fn to_provider_duration(duration: VideoDuration) -> ProviderVideoDuration {
    match duration {
        VideoDuration::Seconds5 => ProviderVideoDuration::Seconds5,
        VideoDuration::Seconds10 => ProviderVideoDuration::Seconds10,
        VideoDuration::Seconds15 => ProviderVideoDuration::Seconds15,
        VideoDuration::Custom(secs) => ProviderVideoDuration::Custom(secs),
    }
}

/// Normalize the prompt to extract text and optional image data.
fn normalize_prompt(prompt: VideoPrompt) -> (String, Option<Vec<u8>>, Option<String>) {
    match prompt {
        VideoPrompt::Text(text) => (text, None, None),
        VideoPrompt::WithImage {
            text,
            image,
            image_content_type,
        } => {
            let (image_data, content_type) = if is_http_url(&image) {
                // URL - will be downloaded by provider
                // For now, return as base64 placeholder (provider handles URLs)
                (Some(image.into_bytes()), image_content_type)
            } else if is_data_url(&image) {
                // Data URL
                let split = split_data_url(&image);
                let base64_content = split.base64_content.unwrap_or_default();
                let data = BASE64.decode(&base64_content).unwrap_or_default();
                let ct = image_content_type.or(split.media_type);
                (Some(data), ct)
            } else {
                // Assume base64 encoded
                let data = BASE64.decode(&image).unwrap_or_else(|_| image.into_bytes());
                (Some(data), image_content_type)
            };
            (text, image_data, content_type)
        }
    }
}

/// Default download helper.
async fn default_download(
    url: &str,
    abort_signal: Option<CancellationToken>,
) -> Result<(Vec<u8>, Option<String>), AIError> {
    let url = reqwest::Url::parse(url).map_err(|e| AIError::InvalidArgument(e.to_string()))?;
    let result = download(url, None, abort_signal)
        .await
        .map_err(|e| AIError::InvalidArgument(e.to_string()))?;
    Ok((result.data, result.media_type))
}

/// Generate videos from a text prompt.
///
/// This function generates videos using a video generation model.
///
/// # Arguments
///
/// * `options` - The generation options including model, prompt, and settings.
///
/// # Returns
///
/// A `GenerateVideoResult` containing the generated videos.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::{generate_video, GenerateVideoOptions, VideoSize, VideoDuration};
///
/// let result = generate_video(GenerateVideoOptions {
///     model: "sora".into(),
///     prompt: "A serene landscape with mountains".into(),
///     size: Some(VideoSize::HD1080p),
///     duration: Some(VideoDuration::Seconds10),
///     ..Default::default()
/// }).await?;
///
/// // Get the first video
/// if let Some(video) = result.video() {
///     println!("Video URL: {:?}", video.as_url());
/// }
/// ```
pub async fn generate_video(options: GenerateVideoOptions) -> Result<GenerateVideoResult, AIError> {
    let model = resolve_video_model(options.model)?;
    let model_id = model.model_id().to_string();

    // Get the number of videos to generate
    let n = options.n.unwrap_or(1);
    let max_videos_per_call = options.max_videos_per_call.unwrap_or(1);

    // Calculate how many API calls we need
    let call_count = n.div_ceil(max_videos_per_call);
    let call_video_counts: Vec<usize> = (0..call_count)
        .map(|i| {
            let remaining = n - i * max_videos_per_call;
            remaining.min(max_videos_per_call)
        })
        .collect();

    // Normalize prompt
    let (prompt_text, image_data, image_content_type) = normalize_prompt(options.prompt.clone());

    // Set up retry configuration
    let retry_config = RetryConfig {
        max_retries: options.max_retries.unwrap_or(2),
        initial_delay_ms: 1000,
        max_delay_ms: 30000,
        multiplier: 2.0,
    };

    // Collect results
    let mut all_videos: Vec<GeneratedVideo> = Vec::new();
    let all_warnings: Vec<Warning> = Vec::new();
    let mut responses: Vec<VideoModelResponseMetadata> = Vec::new();

    // Make API calls
    for call_video_count in call_video_counts {
        // Build call options
        let mut call_options = VideoModelV4CallOptions::new(&prompt_text);
        call_options = call_options.with_n(call_video_count);

        // Set size/resolution
        if let Some(ref resolution) = options.resolution {
            call_options = call_options.with_size(ProviderVideoSize::Custom {
                width: resolution.width,
                height: resolution.height,
            });
        } else if let Some(size) = options.size {
            call_options = call_options.with_size(to_provider_size(size));
        }

        // Set duration
        if let Some(ref duration) = options.duration {
            call_options = call_options.with_duration(to_provider_duration(*duration));
        }

        // Note: FPS and seed are not directly supported by VideoModelV4CallOptions.
        // Provider-specific options should be passed via provider_options.

        // Set style
        if let Some(ref style) = options.style {
            call_options = call_options.with_style(style.clone());
        }

        // Set image if provided
        if let (Some(data), Some(ct)) = (&image_data, &image_content_type) {
            call_options = call_options.with_image(data.clone(), ct);
        }

        // Set provider options
        if let Some(ref provider_opts) = options.provider_options {
            call_options = call_options.with_provider_options(provider_opts.clone());
        }

        // Set abort signal
        if let Some(ref signal) = options.abort_signal {
            call_options = call_options.with_abort_signal(signal.clone());
        }

        // Make the call with retry
        let result = with_retry(retry_config.clone(), options.abort_signal.clone(), || {
            let model = model.clone();
            let opts = call_options.clone();
            async move { model.do_generate_video(opts).await.map_err(AIError::from) }
        })
        .await?;

        // Process videos
        for vid in result.videos {
            let generated = match vid.data {
                VideoData::Url(url) => {
                    // Download the video from URL
                    let (data, media_type) = if let Some(ref download_fn) = options.download {
                        let parsed_url = reqwest::Url::parse(&url)
                            .map_err(|e| AIError::InvalidArgument(e.to_string()))?;
                        let result = download_fn(parsed_url, options.abort_signal.clone())
                            .await
                            .map_err(|e| AIError::InvalidArgument(e.to_string()))?;
                        (result.data, result.media_type)
                    } else {
                        default_download(&url, options.abort_signal.clone()).await?
                    };

                    // Detect media type if not provided
                    let final_media_type = media_type
                        .or_else(|| {
                            detect_media_type(&data, VIDEO_MEDIA_TYPE_SIGNATURES)
                                .map(std::string::ToString::to_string)
                        })
                        .unwrap_or_else(|| "video/mp4".to_string());

                    GeneratedVideo::binary(data).with_content_type(final_media_type)
                }
                VideoData::Base64(data) => {
                    let binary_data = BASE64.decode(&data).unwrap_or_default();
                    let media_type = vid
                        .content_type
                        .clone()
                        .or_else(|| {
                            detect_media_type(&binary_data, VIDEO_MEDIA_TYPE_SIGNATURES)
                                .map(std::string::ToString::to_string)
                        })
                        .unwrap_or_else(|| "video/mp4".to_string());
                    GeneratedVideo::binary(binary_data).with_content_type(media_type)
                }
            };
            all_videos.push(generated);
        }

        // Note: VideoModelV4Result doesn't have a warnings field yet
        // When warnings are supported, uncomment: all_warnings.extend(result.warnings);

        // Add response metadata
        responses.push(VideoModelResponseMetadata::new().with_model_id(&model_id));
    }

    // Check if videos were generated
    if all_videos.is_empty() {
        return Err(AIError::from(NoVideoGeneratedError::with_responses(
            responses,
        )));
    }

    // Build the result
    Ok(GenerateVideoResult::new(all_videos, model_id).with_warnings(all_warnings))
}

#[cfg(test)]
#[path = "generate_video.test.rs"]
mod tests;
