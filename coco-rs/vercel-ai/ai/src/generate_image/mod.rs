//! Generate images from text prompts.
//!
//! This module provides the `generate_image` function for generating images
//! from text prompts using image generation models.

#[allow(clippy::module_inception)]
mod generate_image;
mod image_result;

pub use generate_image::GenerateImageOptions;
pub use generate_image::ImageModel;
pub use generate_image::ImagePrompt;
pub use generate_image::generate_image;
pub use image_result::GenerateImageResult;
pub use image_result::GeneratedImage;
pub use image_result::ImageQuality;
pub use image_result::ImageSize;
pub use image_result::ImageStyle;
