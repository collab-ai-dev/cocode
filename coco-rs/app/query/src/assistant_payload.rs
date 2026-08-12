//! Bounded persistence for provider-generated structured assistant payloads.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use coco_llm_types::AssistantContentPart;
use coco_llm_types::SharedV4FileData;
use uuid::Uuid;

pub(crate) const MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES: usize = 32 * 1024;
pub(crate) const MAX_OPAQUE_STRUCTURED_PART_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ASSISTANT_TURN_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct AssistantPayloadError {
    message: String,
}

impl std::fmt::Display for AssistantPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AssistantPayloadError {}

pub(crate) async fn externalize_assistant_payloads(
    parts: Vec<AssistantContentPart>,
    artifact_root: Option<PathBuf>,
    assistant_uuid: Uuid,
) -> Result<Vec<AssistantContentPart>, AssistantPayloadError> {
    tokio::task::spawn_blocking(move || {
        externalize_assistant_payloads_blocking(parts, artifact_root.as_deref(), assistant_uuid)
    })
    .await
    .map_err(|error| AssistantPayloadError {
        message: format!("assistant payload worker failed: {error}"),
    })?
}

fn externalize_assistant_payloads_blocking(
    mut parts: Vec<AssistantContentPart>,
    artifact_root: Option<&Path>,
    assistant_uuid: Uuid,
) -> Result<Vec<AssistantContentPart>, AssistantPayloadError> {
    for (index, part) in parts.iter_mut().enumerate() {
        match part {
            AssistantContentPart::File(file) => externalize_file_data(
                &mut file.data,
                &file.media_type,
                artifact_root,
                assistant_uuid,
                index,
            )?,
            AssistantContentPart::ReasoningFile(file) => externalize_file_data(
                &mut file.data,
                &file.media_type,
                artifact_root,
                assistant_uuid,
                index,
            )?,
            AssistantContentPart::Custom(_)
            | AssistantContentPart::Source(_)
            | AssistantContentPart::ToolApprovalRequest(_) => {
                validate_structured_part_size(part)?;
            }
            _ => {}
        }

        // Media payloads may have been reduced to a small typed reference,
        // but filenames and provider metadata are independently untrusted.
        if matches!(
            part,
            AssistantContentPart::File(_) | AssistantContentPart::ReasoningFile(_)
        ) {
            validate_structured_part_size(part)?;
        }
    }
    validate_serialized_size(&parts, MAX_ASSISTANT_TURN_BYTES, "provider assistant turn")?;
    Ok(parts)
}

fn externalize_file_data(
    data: &mut SharedV4FileData,
    media_type: &str,
    artifact_root: Option<&Path>,
    assistant_uuid: Uuid,
    index: usize,
) -> Result<(), AssistantPayloadError> {
    let SharedV4FileData::Data { data: raw } = data else {
        return Ok(());
    };
    let base64_len = match raw {
        coco_llm_types::FileRawData::Base64(value) => value.len(),
        coco_llm_types::FileRawData::Bytes(value) => value.len().div_ceil(3).saturating_mul(4),
    };
    if base64_len <= MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES {
        return Ok(());
    }

    let extension = media_extension(media_type);
    let filename = format!("{assistant_uuid}-{index}.{extension}");
    let relative = PathBuf::from("assistant-media").join(&filename);
    let reference = if let Some(root) = artifact_root {
        let bytes = raw.to_bytes().ok_or_else(|| AssistantPayloadError {
            message: format!("provider returned invalid base64 for generated {media_type}"),
        })?;
        let path = root.join(&relative);
        coco_utils_common::write_atomic(&path, bytes).map_err(|error| AssistantPayloadError {
            message: format!(
                "failed to persist generated media {}: {error}",
                path.display()
            ),
        })?;
        relative.to_string_lossy().into_owned()
    } else {
        format!("unavailable/{filename}")
    };
    *data = SharedV4FileData::reference(HashMap::from([("coco".to_string(), reference)]));
    Ok(())
}

fn validate_structured_part_size(part: &AssistantContentPart) -> Result<(), AssistantPayloadError> {
    validate_serialized_size(
        part,
        MAX_OPAQUE_STRUCTURED_PART_BYTES,
        "provider structured content",
    )
}

fn validate_serialized_size<T: serde::Serialize + ?Sized>(
    value: &T,
    limit: usize,
    label: &str,
) -> Result<(), AssistantPayloadError> {
    let mut sink = SerializedSizeLimit {
        written: 0,
        limit,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer(&mut sink, value) {
        if sink.exceeded {
            return Err(AssistantPayloadError {
                message: format!("{label} exceeds the {limit}-byte safety limit"),
            });
        }
        return Err(AssistantPayloadError {
            message: format!("{label} is not serializable: {error}"),
        });
    }
    Ok(())
}

struct SerializedSizeLimit {
    written: usize,
    limit: usize,
    exceeded: bool,
}

impl std::io::Write for SerializedSizeLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self.written.saturating_add(bytes.len());
        if next > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "serialized value exceeds safety limit",
            ));
        }
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn media_extension(media_type: &str) -> &'static str {
    match media_type.split(';').next().unwrap_or(media_type).trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        _ => "bin",
    }
}

#[cfg(test)]
#[path = "assistant_payload.test.rs"]
mod tests;
