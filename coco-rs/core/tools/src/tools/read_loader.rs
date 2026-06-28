//! Shared low-level file loading for ReadTool and background readers.
//!
//! This module deliberately has no ToolUseContext side effects: no
//! FileReadState writes, no nested-memory triggers, and no tool-result
//! presentation. Callers decide what the loaded bytes mean.

use base64::Engine;
use coco_tool_runtime::ToolError;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;

use super::read::ReadInput;

/// Maximum total file size for a FULL read (no `limit`).
pub(crate) const MAX_READ_OUTPUT_BYTES: usize = 256 * 1024;

/// Default output token budget for a read slice.
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 25_000;

/// Upper bound on the RAW image size before decode.
const MAX_IMAGE_DECODE_BYTES: u64 = 32 * 1024 * 1024;

/// Image media-type table for formats we can decode, resize, and send.
const IMAGE_MEDIA_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// Image extensions we recognize but cannot send as multimodal content.
const PLACEHOLDER_IMAGE_EXTENSIONS: &[&str] = &["bmp", "ico", "tiff", "tif", "svg"];

/// Known binary extensions that should not be read as text.
const BINARY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "o", "a", "bin", "class", "pyc", "pyo", "wasm", "zip", "tar",
    "gz", "bz2", "xz", "7z", "rar", "mp3", "mp4", "wav", "avi", "mov", "mkv", "flv", "ttf", "otf",
    "woff", "woff2", "eot", "sqlite", "db",
];

const BLOCKED_DEVICE_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/full",
    "/dev/stdin",
    "/dev/tty",
    "/dev/console",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/fd/0",
    "/dev/fd/1",
    "/dev/fd/2",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFileKind {
    Text,
    SupportedImage { media_type: &'static str },
    PlaceholderImage { extension: String },
    Binary { extension: String },
    Notebook,
    Pdf,
}

#[derive(Debug, Clone)]
pub(crate) struct TextReadSelection {
    pub output: String,
    pub cached_content: String,
    pub range: coco_context::FileReadRange,
    pub num_lines: usize,
    pub start_line: usize,
    pub total_lines: usize,
    pub should_record: bool,
}

#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub base64: String,
    pub media_type: String,
    pub original_size: u64,
    pub original_width: u32,
    pub original_height: u32,
    pub display_width: u32,
    pub display_height: u32,
}

pub fn is_blocked_device_path(file_path: &str) -> bool {
    BLOCKED_DEVICE_PATHS.contains(&file_path)
}

pub fn classify_read_path(path: &Path) -> ReadFileKind {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return ReadFileKind::Text;
    };
    let ext_lower = ext.to_lowercase();
    if let Some(media_type) = IMAGE_MEDIA_TYPES
        .iter()
        .find_map(|(candidate, media_type)| (*candidate == ext_lower).then_some(*media_type))
    {
        return ReadFileKind::SupportedImage { media_type };
    }
    if PLACEHOLDER_IMAGE_EXTENSIONS.contains(&ext_lower.as_str()) {
        return ReadFileKind::PlaceholderImage {
            extension: ext_lower,
        };
    }
    if ext_lower == "ipynb" {
        return ReadFileKind::Notebook;
    }
    if ext_lower == "pdf" {
        return ReadFileKind::Pdf;
    }
    if BINARY_EXTENSIONS.contains(&ext_lower.as_str()) {
        return ReadFileKind::Binary {
            extension: ext_lower,
        };
    }
    ReadFileKind::Text
}

pub fn is_special_read_path(path: &Path) -> bool {
    !matches!(classify_read_path(path), ReadFileKind::Text)
}

pub(crate) fn read_text_selection(
    file_path: &str,
    input: &ReadInput,
) -> Result<TextReadSelection, ToolError> {
    let metadata = std::fs::metadata(file_path).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to stat {file_path}: {e}"),
        display_data: None,
        source: None,
    })?;

    if input.limit.is_some() && metadata.len() > MAX_READ_OUTPUT_BYTES as u64 {
        return read_text_selection_streaming(file_path, input);
    }

    let raw_bytes = std::fs::read(file_path).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to read {file_path}: {e}"),
        display_data: None,
        source: None,
    })?;

    if input.limit.is_none() && raw_bytes.len() > MAX_READ_OUTPUT_BYTES {
        return Err(ToolError::InvalidInput {
            message: format!(
                "File content ({} bytes) exceeds maximum allowed size ({} bytes). \
                 Use the offset and limit parameters to read specific portions of the file.",
                raw_bytes.len(),
                MAX_READ_OUTPUT_BYTES
            ),
            error_code: None,
        });
    }

    let content = decode_text_bytes(file_path, &raw_bytes)?;
    read_text_selection_from_content(file_path, input, content)
}

pub fn read_full_text_for_changed_file(file_path: &Path) -> Result<String, ToolError> {
    let raw_bytes = std::fs::read(file_path).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to read {}: {e}", file_path.display()),
        display_data: None,
        source: None,
    })?;
    decode_text_bytes(&file_path.display().to_string(), &raw_bytes)
}

fn decode_text_bytes(file_path: &str, raw_bytes: &[u8]) -> Result<String, ToolError> {
    let encoding = coco_file_encoding::detect_encoding(raw_bytes);
    encoding
        .decode(raw_bytes)
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to decode {file_path} as {encoding:?}: {e}"),
            display_data: None,
            source: None,
        })
}

fn read_text_selection_from_content(
    file_path: &str,
    input: &ReadInput,
    content: String,
) -> Result<TextReadSelection, ToolError> {
    let offset = normalized_offset(input);
    let explicit_limit = explicit_limit(input);

    if content.is_empty() {
        return Ok(TextReadSelection {
            output: String::new(),
            cached_content: String::new(),
            range: coco_context::FileReadRange::Full,
            num_lines: 0,
            start_line: offset,
            total_lines: 1,
            should_record: false,
        });
    }

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = start_index(offset);

    if start >= total_lines && total_lines > 0 {
        return Ok(TextReadSelection {
            output: String::new(),
            cached_content: String::new(),
            range: coco_context::FileReadRange::Lines {
                offset: if offset > 1 {
                    Some(offset as i32)
                } else {
                    None
                },
                limit: 0,
            },
            num_lines: 0,
            start_line: offset,
            total_lines,
            should_record: false,
        });
    }

    let line_end = explicit_limit
        .map(|limit| start.saturating_add(limit).min(total_lines))
        .unwrap_or(total_lines);
    let slice_bytes: usize = lines[start..line_end].iter().map(|l| l.len() + 1).sum();
    enforce_token_cap(file_path, slice_bytes)?;

    let mut output = String::new();
    for (i, line) in lines[start..line_end].iter().enumerate() {
        let line_num = start + i + 1;
        output.push_str(&format!("{line_num}\t{line}\n"));
    }

    if line_end < total_lines {
        output.push_str(&format!(
            "\n... ({} more lines not shown. Use offset/limit to read more.)",
            total_lines - line_end
        ));
    }

    let requested_line_range = input.limit.is_some() || input.offset.is_some_and(|n| n > 1);
    let range = if requested_line_range {
        coco_context::FileReadRange::Lines {
            offset: if offset > 1 {
                Some(offset as i32)
            } else {
                None
            },
            limit: (line_end - start) as i32,
        }
    } else {
        coco_context::FileReadRange::Full
    };
    let cached_content = if range == coco_context::FileReadRange::Full {
        content
    } else {
        lines[start..line_end].join("\n")
    };

    Ok(TextReadSelection {
        output,
        cached_content,
        range,
        num_lines: line_end - start,
        start_line: if start == 0 { 1 } else { start + 1 },
        total_lines,
        should_record: true,
    })
}

fn read_text_selection_streaming(
    file_path: &str,
    input: &ReadInput,
) -> Result<TextReadSelection, ToolError> {
    reject_unsupported_streaming_encoding(file_path)?;

    let file = File::open(file_path).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to read {file_path}: {e}"),
        display_data: None,
        source: None,
    })?;
    let mut reader = BufReader::new(file);
    let offset = normalized_offset(input);
    let Some(limit) = explicit_limit(input) else {
        return Err(ToolError::InvalidInput {
            message: "streaming range reads require a positive limit".into(),
            error_code: None,
        });
    };
    let start = start_index(offset);
    let requested_end = start.saturating_add(limit);
    let mut line = String::new();
    let mut total_lines = 0usize;
    let mut selected_lines: Vec<String> = Vec::new();
    let mut slice_bytes = 0usize;

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to decode {file_path} as UTF-8 while streaming: {e}"),
                display_data: None,
                source: None,
            })?;
        if bytes == 0 {
            break;
        }
        trim_line_ending(&mut line);
        if total_lines == 0 && line.starts_with('\u{feff}') {
            line.remove(0);
        }
        if total_lines >= start && total_lines < requested_end {
            slice_bytes += line.len() + 1;
            selected_lines.push(line.clone());
        }
        total_lines += 1;
    }

    if start >= total_lines && total_lines > 0 {
        return Ok(TextReadSelection {
            output: String::new(),
            cached_content: String::new(),
            range: coco_context::FileReadRange::Lines {
                offset: if offset > 1 {
                    Some(offset as i32)
                } else {
                    None
                },
                limit: 0,
            },
            num_lines: 0,
            start_line: offset,
            total_lines,
            should_record: false,
        });
    }

    enforce_token_cap(file_path, slice_bytes)?;

    let mut output = String::new();
    for (i, line) in selected_lines.iter().enumerate() {
        let line_num = start + i + 1;
        output.push_str(&format!("{line_num}\t{line}\n"));
    }

    let end = start + selected_lines.len();
    if end < total_lines {
        output.push_str(&format!(
            "\n... ({} more lines not shown. Use offset/limit to read more.)",
            total_lines - end
        ));
    }

    Ok(TextReadSelection {
        output,
        cached_content: selected_lines.join("\n"),
        range: coco_context::FileReadRange::Lines {
            offset: if offset > 1 {
                Some(offset as i32)
            } else {
                None
            },
            limit: selected_lines.len() as i32,
        },
        num_lines: selected_lines.len(),
        start_line: if start == 0 { 1 } else { start + 1 },
        total_lines,
        should_record: true,
    })
}

fn reject_unsupported_streaming_encoding(file_path: &str) -> Result<(), ToolError> {
    let mut file = File::open(file_path).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to read {file_path}: {e}"),
        display_data: None,
        source: None,
    })?;
    let mut prefix = [0u8; 3];
    let bytes = file
        .read(&mut prefix)
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to read {file_path}: {e}"),
            display_data: None,
            source: None,
        })?;
    if bytes >= 2 && (prefix[..2] == [0xff, 0xfe] || prefix[..2] == [0xfe, 0xff]) {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "Cannot range-stream {file_path}: UTF-16 is not supported for large explicit-range reads. \
                 Use a smaller UTF-8 file or convert the file to UTF-8 before reading a range."
            ),
            display_data: None,
            source: None,
        });
    }
    Ok(())
}

fn normalized_offset(input: &ReadInput) -> usize {
    input
        .offset
        .filter(|n| *n >= 0)
        .map(|n| n as usize)
        .unwrap_or(1)
}

fn explicit_limit(input: &ReadInput) -> Option<usize> {
    input.limit.filter(|n| *n > 0).map(|n| n as usize)
}

fn start_index(offset: usize) -> usize {
    if offset == 0 { 0 } else { offset - 1 }
}

fn trim_line_ending(line: &mut String) {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
}

fn enforce_token_cap(file_path: &str, slice_bytes: usize) -> Result<(), ToolError> {
    let token_estimate = slice_bytes / bytes_per_token_for_ext(file_path);
    if token_estimate > DEFAULT_MAX_OUTPUT_TOKENS {
        return Err(ToolError::InvalidInput {
            message: format!(
                "File content ({token_estimate} tokens) exceeds maximum allowed tokens \
                 ({DEFAULT_MAX_OUTPUT_TOKENS}). Use offset and limit parameters to read \
                 specific portions of the file, or search for specific content instead of \
                 reading the whole file."
            ),
            error_code: None,
        });
    }
    Ok(())
}

fn bytes_per_token_for_ext(file_path: &str) -> usize {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "json" | "jsonl" | "jsonc" => 2,
        _ => 4,
    }
}

pub async fn read_image_for_tool(
    file_path: &str,
    media_type: &str,
) -> Result<LoadedImage, ToolError> {
    let metadata =
        tokio::fs::metadata(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to stat image {file_path}: {e}"),
                display_data: None,
                source: None,
            })?;
    if metadata.len() > MAX_IMAGE_DECODE_BYTES {
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "Image file too large to decode: {} bytes > {MAX_IMAGE_DECODE_BYTES} byte \
                 limit. This cap exists to prevent accidentally loading huge files; if you \
                 genuinely need to process a larger image, resize it first with an external \
                 tool (e.g. `magick input.png -resize 2048x2048 output.png`).",
                metadata.len()
            ),
            display_data: None,
            source: None,
        });
    }

    let file_path_owned = file_path.to_string();
    let hint_path = std::path::PathBuf::from(file_path);
    let encoded =
        tokio::task::spawn_blocking(move || -> Result<coco_utils_image::EncodedImage, String> {
            let raw = std::fs::read(&file_path_owned)
                .map_err(|e| format!("failed to read image {file_path_owned}: {e}"))?;
            coco_utils_image::load_for_prompt_bytes(
                &hint_path,
                raw,
                coco_utils_image::PromptImageMode::ResizeToFit,
            )
            .map_err(|e| format!("image processing failed: {e}"))
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("spawn_blocking failed: {e}"),
            display_data: None,
            source: None,
        })?
        .map_err(|e| ToolError::ExecutionFailed {
            message: e,
            display_data: None,
            source: None,
        })?;

    if encoded.mime != media_type {
        tracing::debug!(
            "Image MIME adjusted from filename hint {media_type} to {} after processing",
            encoded.mime
        );
    }

    Ok(LoadedImage {
        base64: base64::engine::general_purpose::STANDARD.encode(&encoded.bytes),
        media_type: encoded.mime,
        original_size: metadata.len(),
        original_width: encoded.original_width,
        original_height: encoded.original_height,
        display_width: encoded.width,
        display_height: encoded.height,
    })
}

#[cfg(test)]
#[path = "read_loader.test.rs"]
mod tests;
