//! Generate videos from text prompts.
//!
//! This module provides the `generate_video` function for generating videos
//! from text prompts using video generation models.

#[allow(clippy::module_inception)]
mod generate_video;
mod video_result;

pub use generate_video::AspectRatio;
pub use generate_video::DownloadFn;
pub use generate_video::GenerateVideoOptions;
pub use generate_video::Resolution;
pub use generate_video::VideoModel;
pub use generate_video::VideoPrompt;
pub use generate_video::generate_video;
pub use video_result::GenerateVideoResult;
pub use video_result::GeneratedVideo;
pub use video_result::VideoData;
pub use video_result::VideoDuration;
pub use video_result::VideoSize;
