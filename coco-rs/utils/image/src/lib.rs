use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::LazyLock;

use crate::error::ImageProcessingError;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use coco_utils_cache::BlockingLruCache;
use coco_utils_cache::sha1_digest;
use image::ColorType;
use image::DynamicImage;
use image::GenericImageView;
use image::ImageEncoder;
use image::ImageFormat;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
/// Maximum width used when resizing images before uploading.
pub const MAX_WIDTH: u32 = 2048;
/// Maximum height used when resizing images before uploading.
pub const MAX_HEIGHT: u32 = 768;

pub mod error;

#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Width of the encoded image as written to `bytes`. For un-resized
    /// images this equals `original_width`; for resized images this is
    /// the post-resize width. Kept as the canonical "current" width so
    /// existing callers continue to work.
    pub width: u32,
    /// Height of the encoded image as written to `bytes` (see `width`).
    pub height: u32,
    /// Original source width before any resize. TS Read tool reports
    /// this as `dimensions.originalWidth` so the model can convert
    /// click coordinates back to the source image's coordinate space.
    pub original_width: u32,
    /// Original source height before any resize.
    pub original_height: u32,
}

impl EncodedImage {
    pub fn into_data_url(self) -> String {
        let encoded = BASE64_STANDARD.encode(&self.bytes);
        format!("data:{};base64,{encoded}", self.mime)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptImageMode {
    ResizeToFit,
    Original,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    digest: [u8; 20],
    mode: PromptImageMode,
}

static IMAGE_CACHE: LazyLock<BlockingLruCache<ImageCacheKey, EncodedImage>> =
    LazyLock::new(|| BlockingLruCache::new(NonZeroUsize::new(32).unwrap_or(NonZeroUsize::MIN)));

pub fn load_for_prompt_bytes(
    path: &Path,
    file_bytes: Vec<u8>,
    mode: PromptImageMode,
) -> Result<EncodedImage, ImageProcessingError> {
    let path_buf = path.to_path_buf();

    let key = ImageCacheKey {
        digest: sha1_digest(&file_bytes),
        mode,
    };

    IMAGE_CACHE.get_or_try_insert_with(key, move || {
        let format = match image::guess_format(&file_bytes) {
            Ok(ImageFormat::Png) => Some(ImageFormat::Png),
            Ok(ImageFormat::Jpeg) => Some(ImageFormat::Jpeg),
            Ok(ImageFormat::Gif) => Some(ImageFormat::Gif),
            Ok(ImageFormat::WebP) => Some(ImageFormat::WebP),
            _ => None,
        };

        let dynamic = image::load_from_memory(&file_bytes)
            .map_err(|source| ImageProcessingError::decode_error(&path_buf, source))?;

        // Original (pre-resize) dimensions — captured before any
        // resize so the EncodedImage can report both original and
        // display sizes to the model. TS `FileReadTool.ts:276-296`
        // surfaces both via the `dimensions` field on image output.
        let (original_width, original_height) = dynamic.dimensions();

        let encoded = if mode == PromptImageMode::Original
            || (original_width <= MAX_WIDTH && original_height <= MAX_HEIGHT)
        {
            // No resize: display dims == original dims.
            if let Some(format) = format.filter(|format| can_preserve_source_bytes(*format)) {
                let mime = format_to_mime(format);
                EncodedImage {
                    bytes: file_bytes,
                    mime,
                    width: original_width,
                    height: original_height,
                    original_width,
                    original_height,
                }
            } else {
                let (bytes, output_format) = encode_image(&dynamic, ImageFormat::Png)?;
                let mime = format_to_mime(output_format);
                EncodedImage {
                    bytes,
                    mime,
                    width: original_width,
                    height: original_height,
                    original_width,
                    original_height,
                }
            }
        } else {
            let resized = dynamic.resize(MAX_WIDTH, MAX_HEIGHT, FilterType::Triangle);
            let target_format = format
                .filter(|format| can_preserve_source_bytes(*format))
                .unwrap_or(ImageFormat::Png);
            let (bytes, output_format) = encode_image(&resized, target_format)?;
            let mime = format_to_mime(output_format);
            EncodedImage {
                bytes,
                mime,
                width: resized.width(),
                height: resized.height(),
                original_width,
                original_height,
            }
        };

        Ok(encoded)
    })
}

/// Normalize raw image bytes for API submission.
///
/// Resizes if the image exceeds `MAX_WIDTH`x`MAX_HEIGHT` and re-encodes
/// to a suitable format. Returns the normalized bytes and MIME type.
pub fn normalize_image_bytes(
    bytes: Vec<u8>,
    source_mime: &str,
) -> Result<(Vec<u8>, String), ImageProcessingError> {
    let dummy_path = match source_mime {
        "image/jpeg" => Path::new("clipboard.jpg"),
        "image/gif" => Path::new("clipboard.gif"),
        "image/webp" => Path::new("clipboard.webp"),
        _ => Path::new("clipboard.png"),
    };
    let encoded = load_for_prompt_bytes(dummy_path, bytes, PromptImageMode::ResizeToFit)?;
    Ok((encoded.bytes, encoded.mime))
}

fn can_preserve_source_bytes(format: ImageFormat) -> bool {
    // Preserve byte-for-byte only for formats we can safely pass through.
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

fn encode_image(
    image: &DynamicImage,
    preferred_format: ImageFormat,
) -> Result<(Vec<u8>, ImageFormat), ImageProcessingError> {
    let target_format = match preferred_format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::WebP => ImageFormat::WebP,
        _ => ImageFormat::Png,
    };

    let mut buffer = Vec::new();

    match target_format {
        ImageFormat::Png => {
            let rgba = image.to_rgba8();
            let encoder = PngEncoder::new(&mut buffer);
            encoder
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgba8.into(),
                )
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        }
        ImageFormat::Jpeg => {
            let mut encoder = JpegEncoder::new_with_quality(&mut buffer, 85);
            encoder
                .encode_image(image)
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        }
        ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            let encoder = WebPEncoder::new_lossless(&mut buffer);
            encoder
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgba8.into(),
                )
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        }
        _ => unreachable!("unsupported target_format should have been handled earlier"),
    }

    Ok((buffer, target_format))
}

fn format_to_mime(format: ImageFormat) -> String {
    match format {
        ImageFormat::Jpeg => "image/jpeg".to_string(),
        ImageFormat::Gif => "image/gif".to_string(),
        ImageFormat::WebP => "image/webp".to_string(),
        _ => "image/png".to_string(),
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
