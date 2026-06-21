//! Resolve a file part's media type to a full `type/subtype` form.
//!
//! For providers whose API requires the full IANA media type (not a top-level
//! segment or a wildcard), this sniffs the subtype from inline bytes when
//! only the top-level was supplied.

use vercel_ai_provider::FilePart;
use vercel_ai_provider::SharedV4FileData;
use vercel_ai_provider::errors::UnsupportedFunctionalityError;

use crate::media_type::get_top_level_media_type;
use crate::media_type::is_full_media_type;

/// Resolve a file part's media type to `type/subtype` form.
///
/// - Returns `media_type` as-is when already full (e.g. `image/png`).
/// - Otherwise, when inline bytes are available, sniffs from magic bytes
///   matched against the top-level segment's signature table.
/// - Errors with `UnsupportedFunctionalityError` when neither path applies.
pub fn resolve_full_media_type(part: &FilePart) -> Result<String, UnsupportedFunctionalityError> {
    if is_full_media_type(&part.media_type) {
        return Ok(part.media_type.clone());
    }

    if let SharedV4FileData::Data { data } = &part.data {
        let bytes = match data.to_bytes() {
            Some(b) => b,
            None => {
                return Err(UnsupportedFunctionalityError::with_message(
                    part.media_type.clone(),
                    format!(
                        "file of media type \"{}\" must specify subtype since base64 decode failed",
                        part.media_type
                    ),
                ));
            }
        };

        let top_level = get_top_level_media_type(&part.media_type);
        if let Some(detected) = detect_by_top_level(&bytes, top_level) {
            return Ok(detected.to_string());
        }

        return Err(UnsupportedFunctionalityError::with_message(
            part.media_type.clone(),
            format!(
                "file of media type \"{}\" must specify subtype since it could not be auto-detected",
                part.media_type
            ),
        ));
    }

    Err(UnsupportedFunctionalityError::with_message(
        part.media_type.clone(),
        format!(
            "file of media type \"{}\" must specify subtype since it is not passed as inline bytes",
            part.media_type
        ),
    ))
}

fn detect_by_top_level(bytes: &[u8], top_level: &str) -> Option<&'static str> {
    match top_level {
        "image" => detect_image(bytes),
        "audio" => detect_audio(bytes),
        "video" => detect_video(bytes),
        "application" => detect_document(bytes),
        _ => None,
    }
}

fn matches(bytes: &[u8], prefix: &[Option<u8>]) -> bool {
    bytes.len() >= prefix.len()
        && prefix
            .iter()
            .enumerate()
            .all(|(i, p)| p.is_none_or(|b| bytes[i] == b))
}

fn detect_image(bytes: &[u8]) -> Option<&'static str> {
    let bytes = strip_id3_tags_if_present(bytes);
    const SIGS: &[(&[Option<u8>], &str)] = &[
        (&[Some(0x47), Some(0x49), Some(0x46)], "image/gif"),
        (
            &[Some(0x89), Some(0x50), Some(0x4e), Some(0x47)],
            "image/png",
        ),
        (&[Some(0xff), Some(0xd8)], "image/jpeg"),
        (
            &[
                Some(0x52),
                Some(0x49),
                Some(0x46),
                Some(0x46),
                None,
                None,
                None,
                None,
                Some(0x57),
                Some(0x45),
                Some(0x42),
                Some(0x50),
            ],
            "image/webp",
        ),
        (&[Some(0x42), Some(0x4d)], "image/bmp"),
        (
            &[Some(0x49), Some(0x49), Some(0x2a), Some(0x00)],
            "image/tiff",
        ),
        (
            &[Some(0x4d), Some(0x4d), Some(0x00), Some(0x2a)],
            "image/tiff",
        ),
        (
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
            "image/avif",
        ),
        (
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
            "image/heic",
        ),
    ];
    SIGS.iter()
        .find(|(p, _)| matches(&bytes, p))
        .map(|(_, m)| *m)
}

fn detect_audio(bytes: &[u8]) -> Option<&'static str> {
    let bytes = strip_id3_tags_if_present(bytes);
    const SIGS: &[(&[Option<u8>], &str)] = &[
        (&[Some(0xff), Some(0xfb)], "audio/mpeg"),
        (&[Some(0xff), Some(0xfa)], "audio/mpeg"),
        (&[Some(0xff), Some(0xf3)], "audio/mpeg"),
        (&[Some(0xff), Some(0xf2)], "audio/mpeg"),
        (&[Some(0xff), Some(0xe3)], "audio/mpeg"),
        (&[Some(0xff), Some(0xe2)], "audio/mpeg"),
        (
            &[
                Some(0x52),
                Some(0x49),
                Some(0x46),
                Some(0x46),
                None,
                None,
                None,
                None,
                Some(0x57),
                Some(0x41),
                Some(0x56),
                Some(0x45),
            ],
            "audio/wav",
        ),
        (
            &[Some(0x4f), Some(0x67), Some(0x67), Some(0x53)],
            "audio/ogg",
        ),
        (
            &[Some(0x66), Some(0x4c), Some(0x61), Some(0x43)],
            "audio/flac",
        ),
        (
            &[Some(0x40), Some(0x15), Some(0x00), Some(0x00)],
            "audio/aac",
        ),
        (
            &[Some(0x66), Some(0x74), Some(0x79), Some(0x70)],
            "audio/mp4",
        ),
        (
            &[Some(0x1a), Some(0x45), Some(0xdf), Some(0xa3)],
            "audio/webm",
        ),
    ];
    SIGS.iter()
        .find(|(p, _)| matches(&bytes, p))
        .map(|(_, m)| *m)
}

fn detect_video(bytes: &[u8]) -> Option<&'static str> {
    const SIGS: &[(&[Option<u8>], &str)] = &[
        (
            &[
                Some(0x00),
                Some(0x00),
                Some(0x00),
                None,
                Some(0x66),
                Some(0x74),
                Some(0x79),
                Some(0x70),
            ],
            "video/mp4",
        ),
        (
            &[Some(0x1a), Some(0x45), Some(0xdf), Some(0xa3)],
            "video/webm",
        ),
        (
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
                Some(0x74),
            ],
            "video/quicktime",
        ),
        (
            &[Some(0x52), Some(0x49), Some(0x46), Some(0x46)],
            "video/x-msvideo",
        ),
    ];
    SIGS.iter()
        .find(|(p, _)| matches(bytes, p))
        .map(|(_, m)| *m)
}

fn detect_document(bytes: &[u8]) -> Option<&'static str> {
    const SIGS: &[(&[Option<u8>], &str)] = &[(
        &[Some(0x25), Some(0x50), Some(0x44), Some(0x46)],
        "application/pdf",
    )];
    SIGS.iter()
        .find(|(p, _)| matches(bytes, p))
        .map(|(_, m)| *m)
}

/// Strip ID3v2 tag if present at the start of an MP3 / audio byte stream.
/// Returns a borrowed slice when no tag is found, owned bytes otherwise.
fn strip_id3_tags_if_present(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if bytes.len() > 10 && bytes[0] == 0x49 && bytes[1] == 0x44 && bytes[2] == 0x33 {
        let id3_size = (((bytes[6] & 0x7f) as usize) << 21)
            | (((bytes[7] & 0x7f) as usize) << 14)
            | (((bytes[8] & 0x7f) as usize) << 7)
            | ((bytes[9] & 0x7f) as usize);
        let offset = id3_size + 10;
        if offset < bytes.len() {
            return std::borrow::Cow::Owned(bytes[offset..].to_vec());
        }
    }
    std::borrow::Cow::Borrowed(bytes)
}

#[cfg(test)]
#[path = "resolve_full_media_type.test.rs"]
mod tests;
