//! Detect media type from file signatures (magic bytes).
//!
//! This module provides functionality to detect the MIME type of binary data
//! by examining file signatures (magic bytes) at the beginning of the data.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// A media type signature for detecting file types.
#[derive(Debug, Clone)]
pub struct MediaTypeSignature {
    /// The IANA media type (MIME type).
    pub media_type: &'static str,
    /// The expected bytes prefix. `None` values act as wildcards.
    pub bytes_prefix: &'static [Option<u8>],
}

impl MediaTypeSignature {
    /// Create a new signature.
    pub const fn new(media_type: &'static str, bytes_prefix: &'static [Option<u8>]) -> Self {
        Self {
            media_type,
            bytes_prefix,
        }
    }

    /// Check if the given data matches this signature.
    pub fn matches(&self, data: &[u8]) -> bool {
        if data.len() < self.bytes_prefix.len() {
            return false;
        }
        self.bytes_prefix
            .iter()
            .enumerate()
            .all(|(i, byte)| match byte {
                None => true,
                Some(b) => data[i] == *b,
            })
    }
}

/// Image media type signatures.
pub static IMAGE_MEDIA_TYPE_SIGNATURES: &[MediaTypeSignature] = &[
    MediaTypeSignature::new("image/gif", &[Some(0x47), Some(0x49), Some(0x46)]), // GIF
    MediaTypeSignature::new(
        "image/png",
        &[Some(0x89), Some(0x50), Some(0x4e), Some(0x47)],
    ), // PNG
    MediaTypeSignature::new("image/jpeg", &[Some(0xff), Some(0xd8)]),            // JPEG
    MediaTypeSignature::new(
        "image/webp",
        &[
            Some(0x52),
            Some(0x49),
            Some(0x46),
            Some(0x46), // "RIFF"
            None,
            None,
            None,
            None, // file size (variable)
            Some(0x57),
            Some(0x45),
            Some(0x42),
            Some(0x50), // "WEBP"
        ],
    ),
    MediaTypeSignature::new("image/bmp", &[Some(0x42), Some(0x4d)]), // BMP
    MediaTypeSignature::new(
        "image/tiff",
        &[Some(0x49), Some(0x49), Some(0x2a), Some(0x00)],
    ), // TIFF (little-endian)
    MediaTypeSignature::new(
        "image/tiff",
        &[Some(0x4d), Some(0x4d), Some(0x00), Some(0x2a)],
    ), // TIFF (big-endian)
    MediaTypeSignature::new(
        "image/avif",
        &[
            Some(0x00),
            Some(0x00),
            Some(0x00),
            Some(0x20),
            Some(0x66),
            Some(0x74),
            Some(0x79),
            Some(0x70),
            Some(0x61),
            Some(0x76),
            Some(0x69),
            Some(0x66),
        ],
    ),
    MediaTypeSignature::new(
        "image/heic",
        &[
            Some(0x00),
            Some(0x00),
            Some(0x00),
            Some(0x20),
            Some(0x66),
            Some(0x74),
            Some(0x79),
            Some(0x70),
            Some(0x68),
            Some(0x65),
            Some(0x69),
            Some(0x63),
        ],
    ),
];

/// Audio media type signatures.
pub static AUDIO_MEDIA_TYPE_SIGNATURES: &[MediaTypeSignature] = &[
    MediaTypeSignature::new("audio/mpeg", &[Some(0xff), Some(0xfb)]),
    MediaTypeSignature::new("audio/mpeg", &[Some(0xff), Some(0xfa)]),
    MediaTypeSignature::new("audio/mpeg", &[Some(0xff), Some(0xf3)]),
    MediaTypeSignature::new("audio/mpeg", &[Some(0xff), Some(0xf2)]),
    MediaTypeSignature::new("audio/mpeg", &[Some(0xff), Some(0xe3)]),
    MediaTypeSignature::new("audio/mpeg", &[Some(0xff), Some(0xe2)]),
    MediaTypeSignature::new(
        "audio/wav",
        &[
            Some(0x52), // R
            Some(0x49), // I
            Some(0x46), // F
            Some(0x46), // F
            None,
            None,
            None,
            None,
            Some(0x57), // W
            Some(0x41), // A
            Some(0x56), // V
            Some(0x45), // E
        ],
    ),
    MediaTypeSignature::new(
        "audio/ogg",
        &[Some(0x4f), Some(0x67), Some(0x67), Some(0x53)],
    ),
    MediaTypeSignature::new(
        "audio/flac",
        &[Some(0x66), Some(0x4c), Some(0x61), Some(0x43)],
    ),
    MediaTypeSignature::new(
        "audio/aac",
        &[Some(0x40), Some(0x15), Some(0x00), Some(0x00)],
    ),
    MediaTypeSignature::new(
        "audio/mp4",
        &[Some(0x66), Some(0x74), Some(0x79), Some(0x70)],
    ),
    MediaTypeSignature::new(
        "audio/webm",
        &[Some(0x1a), Some(0x45), Some(0xdf), Some(0xa3)],
    ),
];

/// Video media type signatures.
pub static VIDEO_MEDIA_TYPE_SIGNATURES: &[MediaTypeSignature] = &[
    MediaTypeSignature::new(
        "video/mp4",
        &[
            Some(0x00),
            Some(0x00),
            Some(0x00),
            None,
            Some(0x66),
            Some(0x74),
            Some(0x79),
            Some(0x70), // ftyp
        ],
    ),
    MediaTypeSignature::new(
        "video/webm",
        &[Some(0x1a), Some(0x45), Some(0xdf), Some(0xa3)], // EBML
    ),
    MediaTypeSignature::new(
        "video/quicktime",
        &[
            Some(0x00),
            Some(0x00),
            Some(0x00),
            Some(0x14),
            Some(0x66),
            Some(0x74),
            Some(0x79),
            Some(0x70),
            Some(0x71),
            Some(0x74), // ftypqt
        ],
    ),
    MediaTypeSignature::new(
        "video/x-msvideo",
        &[Some(0x52), Some(0x49), Some(0x46), Some(0x46)], // RIFF (AVI)
    ),
];

/// Strip ID3 tags from MP3 data if present.
fn strip_id3_tags(data: &[u8]) -> Vec<u8> {
    // Check for ID3v2 header: "ID3"
    if data.len() > 10 && data[0] == 0x49 && data[1] == 0x44 && data[2] == 0x33 {
        // ID3v2 tag size is stored as a syncsafe integer in bytes 6-9
        let id3_size = ((data[6] as usize & 0x7f) << 21)
            | ((data[7] as usize & 0x7f) << 14)
            | ((data[8] as usize & 0x7f) << 7)
            | (data[9] as usize & 0x7f);
        // Return data after ID3 tag (header is 10 bytes)
        return data[id3_size + 10..].to_vec();
    }
    data.to_vec()
}

/// Check if data has ID3 tags.
fn has_id3_tags(data: &[u8]) -> bool {
    data.len() > 10 && data[0] == 0x49 && data[1] == 0x44 && data[2] == 0x33 // "ID3"
}

/// Detect the media type of binary data using file signatures.
///
/// # Arguments
///
/// * `data` - The binary data to detect (can be raw bytes or base64-encoded string)
/// * `signatures` - The signatures to use for detection (image, audio, or video)
///
/// # Returns
///
/// The detected media type, or `None` if no signature matched.
///
/// # Example
///
/// ```
/// use vercel_ai::util::detect_media_type::{detect_media_type, IMAGE_MEDIA_TYPE_SIGNATURES};
///
/// let png_data = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
/// let media_type = detect_media_type(&png_data, IMAGE_MEDIA_TYPE_SIGNATURES);
/// assert_eq!(media_type, Some("image/png"));
/// ```
pub fn detect_media_type(data: &[u8], signatures: &[MediaTypeSignature]) -> Option<&'static str> {
    // Strip ID3 tags if present (for MP3 files)
    let processed_data = if has_id3_tags(data) {
        strip_id3_tags(data)
    } else {
        data.to_vec()
    };

    for signature in signatures {
        if signature.matches(&processed_data) {
            return Some(signature.media_type);
        }
    }
    None
}

/// Detect media type from base64-encoded string.
///
/// This is a convenience wrapper that decodes the base64 string and then
/// detects the media type.
pub fn detect_media_type_from_base64(
    base64_data: &str,
    signatures: &[MediaTypeSignature],
) -> Option<&'static str> {
    // Decode only the first ~24 bytes (32 base64 chars) for detection
    let detection_len = (base64_data.len().min(32) / 4) * 4; // Must be multiple of 4
    let truncated = &base64_data[..detection_len];

    let decoded = BASE64.decode(truncated).ok()?;
    detect_media_type(&decoded, signatures)
}

/// Detect media type from either raw bytes or base64 string.
///
/// If `data` is a string, it's treated as base64-encoded data.
/// If `data` is bytes, it's used directly.
pub fn detect_media_type_auto(
    data: &[u8],
    signatures: &[MediaTypeSignature],
) -> Option<&'static str> {
    detect_media_type(data, signatures)
}

#[cfg(test)]
#[path = "detect_media_type.test.rs"]
mod tests;
