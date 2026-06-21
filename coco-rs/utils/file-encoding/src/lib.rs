//! File encoding and line ending detection and preservation utilities.
//!
//! This crate provides utilities to detect and preserve file encodings (UTF-8, UTF-16LE, UTF-16BE)
//! and line endings (LF, CRLF, CR) when reading and writing files.
//!
//! # Example
//!
//! ```no_run
//! use coco_file_encoding::{detect_encoding, detect_line_ending, write_with_format, Encoding, LineEnding};
//! use std::path::Path;
//!
//! // Detect encoding from raw bytes
//! let bytes = std::fs::read("file.txt").unwrap();
//! let encoding = detect_encoding(&bytes);
//!
//! // Decode content
//! let content = encoding.decode(&bytes).unwrap();
//!
//! // Detect line ending from content
//! let line_ending = detect_line_ending(&content);
//!
//! // Write back preserving format
//! write_with_format(Path::new("file.txt"), &content, encoding, line_ending).unwrap();
//! ```

use std::io;
use std::path::Path;

/// File encoding type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// UTF-8 encoding (default).
    #[default]
    Utf8,
    /// UTF-8 encoding with BOM (EF BB BF).
    Utf8WithBom,
    /// UTF-16 Little Endian encoding.
    Utf16Le,
    /// UTF-16 Big Endian encoding.
    Utf16Be,
}

impl Encoding {
    /// Decode bytes to string using this encoding.
    pub fn decode(&self, bytes: &[u8]) -> Result<String, EncodingError> {
        match self {
            Encoding::Utf8 | Encoding::Utf8WithBom => {
                // Skip BOM if present
                let content = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                    &bytes[3..]
                } else {
                    bytes
                };
                String::from_utf8(content.to_vec())
                    .map_err(|e| EncodingError::InvalidUtf8(e.to_string()))
            }
            Encoding::Utf16Le => {
                // Skip BOM if present
                let content = if bytes.starts_with(&[0xFF, 0xFE]) {
                    &bytes[2..]
                } else {
                    bytes
                };
                decode_utf16le(content)
            }
            Encoding::Utf16Be => {
                // Skip BOM if present
                let content = if bytes.starts_with(&[0xFE, 0xFF]) {
                    &bytes[2..]
                } else {
                    bytes
                };
                decode_utf16be(content)
            }
        }
    }

    /// Encode string to bytes using this encoding.
    pub fn encode(&self, content: &str) -> Vec<u8> {
        match self {
            Encoding::Utf8 | Encoding::Utf8WithBom => content.as_bytes().to_vec(),
            Encoding::Utf16Le => encode_utf16le(content),
            Encoding::Utf16Be => encode_utf16be(content),
        }
    }

    /// Returns whether this encoding should include a BOM when writing.
    /// UTF-16 files typically include BOM for proper detection.
    /// UTF-8 with BOM preserves the original BOM.
    pub fn bom(&self) -> &'static [u8] {
        match self {
            Encoding::Utf8 => &[],
            Encoding::Utf8WithBom => &[0xEF, 0xBB, 0xBF],
            Encoding::Utf16Le => &[0xFF, 0xFE],
            Encoding::Utf16Be => &[0xFE, 0xFF],
        }
    }
}

/// Line ending type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// Unix-style line feed (LF, \n) - default.
    #[default]
    Lf,
    /// Windows-style carriage return + line feed (CRLF, \r\n).
    CrLf,
    /// Classic Mac-style carriage return (CR, \r).
    Cr,
}

impl LineEnding {
    /// Returns the string representation of this line ending.
    pub fn as_str(&self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// Encoding-related errors.
#[derive(Debug)]
pub enum EncodingError {
    /// Invalid UTF-8 sequence.
    InvalidUtf8(String),
    /// Invalid UTF-16 sequence.
    InvalidUtf16(String),
    /// I/O error.
    Io(io::Error),
}

impl std::fmt::Display for EncodingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodingError::InvalidUtf8(msg) => write!(f, "Invalid UTF-8: {msg}"),
            EncodingError::InvalidUtf16(msg) => write!(f, "Invalid UTF-16: {msg}"),
            EncodingError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for EncodingError {}

impl From<io::Error> for EncodingError {
    fn from(e: io::Error) -> Self {
        EncodingError::Io(e)
    }
}

/// Detect file encoding from raw bytes by checking for BOM.
///
/// Detection priority:
/// 1. UTF-16LE BOM (FF FE)
/// 2. UTF-16BE BOM (FE FF)
/// 3. UTF-8 BOM (EF BB BF) - returns Utf8WithBom to preserve BOM
/// 4. Default to UTF-8 (no BOM)
pub fn detect_encoding(bytes: &[u8]) -> Encoding {
    if bytes.len() >= 2 {
        // Check UTF-16 BOMs first
        if bytes.starts_with(&[0xFF, 0xFE]) {
            return Encoding::Utf16Le;
        }
        if bytes.starts_with(&[0xFE, 0xFF]) {
            return Encoding::Utf16Be;
        }
    }
    // Check UTF-8 BOM - preserve it by returning Utf8WithBom
    if bytes.len() >= 3 && bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Encoding::Utf8WithBom;
    }
    // Default to UTF-8 (no BOM)
    Encoding::Utf8
}

/// Detect line ending style from string content.
///
/// Uses simple heuristic aligned with Claude Code: if content contains CRLF,
/// treat as CRLF; otherwise LF. This works for 99% of cases.
pub fn detect_line_ending(content: &str) -> LineEnding {
    if content.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

/// Check if content has a trailing newline.
pub fn has_trailing_newline(content: &str) -> bool {
    content.ends_with('\n')
}

/// Preserve trailing newline state from original content.
///
/// If original had a trailing newline and modified doesn't, add one.
/// If original didn't have a trailing newline and modified does, remove it.
/// This prevents spurious diffs from trailing newline changes.
pub fn preserve_trailing_newline(original: &str, modified: &str) -> String {
    let had_trailing = original.ends_with('\n');
    let has_trailing = modified.ends_with('\n');

    match (had_trailing, has_trailing) {
        (true, false) => format!("{modified}\n"),
        (false, true) => modified.trim_end_matches('\n').to_string(),
        _ => modified.to_string(),
    }
}

/// Normalize line endings in content to the specified format.
///
/// Converts all line endings (CRLF, CR, LF) to the target format.
pub fn normalize_line_endings(content: &str, target: LineEnding) -> String {
    // First normalize everything to LF
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");

    // Then convert to target
    match target {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
        LineEnding::Cr => normalized.replace('\n', "\r"),
    }
}

/// Read a file and detect its encoding and line ending.
///
/// Returns the decoded content, detected encoding, and detected line ending.
pub fn read_with_format(path: &Path) -> Result<(String, Encoding, LineEnding), EncodingError> {
    let bytes = std::fs::read(path)?;
    let encoding = detect_encoding(&bytes);
    let content = encoding.decode(&bytes)?;
    let line_ending = detect_line_ending(&content);
    Ok((content, encoding, line_ending))
}

/// Write content to a file with the specified encoding and line ending.
///
/// Normalizes line endings to the target format before writing.
pub fn write_with_format(
    path: &Path,
    content: &str,
    encoding: Encoding,
    line_ending: LineEnding,
) -> Result<(), EncodingError> {
    // Normalize line endings
    let normalized = normalize_line_endings(content, line_ending);

    // Encode content
    let mut bytes = encoding.bom().to_vec();
    bytes.extend(encoding.encode(&normalized));

    std::fs::write(path, bytes)?;
    Ok(())
}

/// Async version: Read a file and detect its encoding and line ending.
pub async fn read_with_format_async(
    path: &Path,
) -> Result<(String, Encoding, LineEnding), EncodingError> {
    let bytes = tokio::fs::read(path).await?;
    let encoding = detect_encoding(&bytes);
    let content = encoding.decode(&bytes)?;
    let line_ending = detect_line_ending(&content);
    Ok((content, encoding, line_ending))
}

/// Async version: Write content to a file with the specified encoding and line ending.
pub async fn write_with_format_async(
    path: &Path,
    content: &str,
    encoding: Encoding,
    line_ending: LineEnding,
) -> Result<(), EncodingError> {
    // Normalize line endings
    let normalized = normalize_line_endings(content, line_ending);

    // Encode content
    let mut bytes = encoding.bom().to_vec();
    bytes.extend(encoding.encode(&normalized));

    tokio::fs::write(path, bytes).await?;
    Ok(())
}

// Internal: Decode UTF-16LE bytes to String
fn decode_utf16le(bytes: &[u8]) -> Result<String, EncodingError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(EncodingError::InvalidUtf16(
            "UTF-16LE requires even number of bytes".to_string(),
        ));
    }
    let u16_iter = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]));
    char::decode_utf16(u16_iter)
        .collect::<Result<String, _>>()
        .map_err(|e| EncodingError::InvalidUtf16(e.to_string()))
}

// Internal: Decode UTF-16BE bytes to String
fn decode_utf16be(bytes: &[u8]) -> Result<String, EncodingError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(EncodingError::InvalidUtf16(
            "UTF-16BE requires even number of bytes".to_string(),
        ));
    }
    let u16_iter = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
    char::decode_utf16(u16_iter)
        .collect::<Result<String, _>>()
        .map_err(|e| EncodingError::InvalidUtf16(e.to_string()))
}

// Internal: Encode String to UTF-16LE bytes
fn encode_utf16le(content: &str) -> Vec<u8> {
    content.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

// Internal: Encode String to UTF-16BE bytes
fn encode_utf16be(content: &str) -> Vec<u8> {
    content.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
