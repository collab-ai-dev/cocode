use super::*;
use std::sync::Arc;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::VideoModelV4;
use vercel_ai_provider::VideoModelV4Result;

// Mock video model for testing
struct MockVideoModel {
    id: String,
}

#[async_trait::async_trait]
impl VideoModelV4 for MockVideoModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        &self.id
    }

    async fn do_generate_video(
        &self,
        _options: VideoModelV4CallOptions,
    ) -> Result<VideoModelV4Result, AISdkError> {
        Ok(VideoModelV4Result::from_urls(vec![
            "https://example.com/video1.mp4".to_string(),
        ]))
    }
}

#[test]
fn test_video_model_from_id() {
    let model: VideoModel = VideoModel::from_id("test-model");
    assert!(model.is_string());
}

#[test]
fn test_video_model_from_v4() {
    let mock = Arc::new(MockVideoModel {
        id: "test-model".to_string(),
    });
    let model: VideoModel = VideoModel::from_v4(mock);
    assert!(!model.is_string());
}

#[test]
fn test_video_size_dimensions() {
    assert_eq!(VideoSize::HD720p.dimensions(), (1280, 720));
    assert_eq!(VideoSize::HD1080p.dimensions(), (1920, 1080));
    assert_eq!(VideoSize::UHD4K.dimensions(), (3840, 2160));
    assert_eq!(VideoSize::Square.dimensions(), (1080, 1080));
    assert_eq!(VideoSize::Portrait.dimensions(), (1080, 1920));
    assert_eq!(
        VideoSize::Custom {
            width: 800,
            height: 600
        }
        .dimensions(),
        (800, 600)
    );
}

#[test]
fn test_video_size_parse() {
    assert_eq!(
        VideoSize::parse("1920x1080"),
        Some(VideoSize::Custom {
            width: 1920,
            height: 1080
        })
    );
    assert_eq!(VideoSize::parse("invalid"), None);
    assert_eq!(VideoSize::parse("1920x"), None);
}

#[test]
fn test_video_size_display() {
    assert_eq!(format!("{}", VideoSize::HD1080p), "1920x1080");
    assert_eq!(
        format!(
            "{}",
            VideoSize::Custom {
                width: 800,
                height: 600
            }
        ),
        "800x600"
    );
}

#[test]
fn test_video_duration_seconds() {
    assert_eq!(VideoDuration::Seconds5.seconds(), 5);
    assert_eq!(VideoDuration::Seconds10.seconds(), 10);
    assert_eq!(VideoDuration::Seconds15.seconds(), 15);
    assert_eq!(VideoDuration::Custom(30).seconds(), 30);
}

#[test]
fn test_generated_video_url() {
    let video = GeneratedVideo::url("https://example.com/video.mp4");
    assert_eq!(video.as_url(), Some("https://example.com/video.mp4"));
    assert!(video.as_base64().is_none());
    assert!(video.content_type.is_none());
}

#[test]
fn test_generated_video_base64() {
    let video = GeneratedVideo::base64("dmlkZW8gZGF0YQ==").with_content_type("video/mp4");
    assert_eq!(video.as_base64(), Some("dmlkZW8gZGF0YQ=="));
    assert!(video.as_url().is_none());
    assert_eq!(video.content_type, Some("video/mp4".to_string()));
}

#[test]
fn test_generated_video_extension() {
    let mp4 = GeneratedVideo::url("test").with_content_type("video/mp4");
    assert_eq!(mp4.extension(), "mp4");

    let webm = GeneratedVideo::url("test").with_content_type("video/webm");
    assert_eq!(webm.extension(), "webm");

    let unknown = GeneratedVideo::url("test");
    assert_eq!(unknown.extension(), "bin");
}

#[test]
fn test_generate_video_options() {
    let options = GenerateVideoOptions::new("test-model", "A test video")
        .with_n(2)
        .with_size(VideoSize::HD1080p)
        .with_duration(VideoDuration::Seconds10)
        .with_style("cinematic");

    assert!(options.model.is_string());
    assert_eq!(options.n, Some(2));
    assert_eq!(options.size, Some(VideoSize::HD1080p));
    assert_eq!(options.duration, Some(VideoDuration::Seconds10));
    assert_eq!(options.style, Some("cinematic".to_string()));
}

#[test]
fn test_video_prompt() {
    let text_prompt: VideoPrompt = "A test video".into();
    assert!(matches!(text_prompt, VideoPrompt::Text(_)));

    let image_prompt = VideoPrompt::WithImage {
        text: "A test video".to_string(),
        image: "https://example.com/image.jpg".to_string(),
        image_content_type: Some("image/jpeg".to_string()),
    };
    assert!(matches!(image_prompt, VideoPrompt::WithImage { .. }));
}
