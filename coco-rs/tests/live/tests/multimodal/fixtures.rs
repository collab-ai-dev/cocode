//! Real (decodable) image bytes for the multimodal suite. 1×1 pixel
//! encodings keep fixtures tiny — valid enough that the `image` crate
//! round-trips them through ReadTool's resize+re-encode pipeline.

use anyhow::Context;
use anyhow::Result;
use image::ColorType;
use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;

/// 1×1 opaque-red PNG. Returns the raw bytes as the `image` crate
/// would produce them — i.e. a real PNG header, IDAT chunk, and IEND.
pub fn png_1x1_red() -> Result<Vec<u8>> {
    let pixel = [255u8, 0, 0, 255]; // RGBA8
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&pixel, 1, 1, ColorType::Rgba8.into())
        .context("encode 1x1 PNG")?;
    Ok(out)
}

/// 1×1 opaque-red JPEG. Quality 85 matches the encoder default in
/// `utils/image`.
pub fn jpeg_1x1_red() -> Result<Vec<u8>> {
    let pixel = [255u8, 0, 0]; // JPEG has no alpha
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 85)
        .write_image(&pixel, 1, 1, ColorType::Rgb8.into())
        .context("encode 1x1 JPEG")?;
    Ok(out)
}

/// 1×1 lossless WebP — exercises the WebP-specific decode path inside
/// the ReadTool image pipeline (D1: WebP-with-alpha is downgraded to
/// PNG, so we expect ReadTool to pick a media type).
pub fn webp_1x1_red() -> Result<Vec<u8>> {
    let pixel = [255u8, 0, 0, 255];
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(&pixel, 1, 1, ColorType::Rgba8.into())
        .context("encode 1x1 WebP")?;
    Ok(out)
}
