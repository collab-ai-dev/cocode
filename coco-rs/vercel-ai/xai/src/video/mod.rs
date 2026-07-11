//! xAI video generation surface (`grok-imagine-video` family).
//!
//! Async create → poll flow: `POST /videos/generations` (or `/videos/edits` /
//! `/videos/extensions`) then `GET /videos/{request_id}` until completion.
//! Port of `xai-video-model.ts` / `xai-video-model-options.ts`.

pub mod xai_video_model;
pub mod xai_video_options;

pub use xai_video_model::XaiVideoModel;
pub use xai_video_options::XaiVideoMode;
pub use xai_video_options::XaiVideoProviderOptions;
pub use xai_video_options::XaiVideoResolution;
pub use xai_video_options::extract_xai_video_options;
pub use xai_video_options::resolve_video_mode;
