//! xAI image generation surface (`grok-imagine-image` family).
//!
//! Text-to-image via `POST /images/generations`; image editing via
//! `POST /images/edits` when input files are provided. Port of
//! `xai-image-model.ts` / `xai-image-model-options.ts`.

pub mod xai_image_model;
pub mod xai_image_options;

pub use xai_image_model::XaiImageModel;
pub use xai_image_model::XaiImageResponse;
pub use xai_image_options::XaiImageProviderOptions;
pub use xai_image_options::XaiImageQuality;
pub use xai_image_options::XaiImageResolution;
pub use xai_image_options::extract_xai_image_options;
