//! Bounded persistence for provider-generated structured assistant payloads.

use std::collections::HashMap;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use base64::Engine as _;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::SharedV4FileData;
use coco_llm_types::ToolResultContent;
use coco_llm_types::ToolResultContentPart;
use uuid::Uuid;

pub(crate) const MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES: usize = 32 * 1024;
pub(crate) const MAX_OPAQUE_STRUCTURED_PART_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ASSISTANT_TURN_BYTES: usize =
    coco_inference::MAX_STREAMED_ASSISTANT_TURN_BYTES;
const COCO_ARTIFACT_REFERENCE: &str = "coco";
const COCO_ARTIFACT_MEDIA_TYPE: &str = "coco-media-type";

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

pub(crate) async fn rehydrate_assistant_payloads(
    mut prompt: Vec<LlmMessage>,
    artifact_root: Option<PathBuf>,
) -> Result<Vec<LlmMessage>, AssistantPayloadError> {
    tokio::task::spawn_blocking(move || {
        let mut budget = RehydrationBudget::default();
        for message in &mut prompt {
            let LlmMessage::Assistant { content, .. } = message else {
                continue;
            };
            for part in content {
                match part {
                    AssistantContentPart::File(file) => {
                        rehydrate_file_data(&mut file.data, artifact_root.as_deref(), &mut budget)?;
                    }
                    AssistantContentPart::ReasoningFile(file) => {
                        rehydrate_file_data(&mut file.data, artifact_root.as_deref(), &mut budget)?;
                    }
                    AssistantContentPart::ToolResult(result) => {
                        rehydrate_tool_result(
                            &mut result.output,
                            artifact_root.as_deref(),
                            &mut budget,
                        )?;
                    }
                    _ => {}
                }
            }
        }
        Ok(prompt)
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
    let mut created_artifacts = Vec::new();
    let result = (|| {
        for (index, part) in parts.iter_mut().enumerate() {
            match part {
                AssistantContentPart::File(file) => externalize_file_data(
                    &mut file.data,
                    &file.media_type,
                    artifact_root,
                    assistant_uuid,
                    index,
                    &mut created_artifacts,
                )?,
                AssistantContentPart::ReasoningFile(file) => externalize_file_data(
                    &mut file.data,
                    &file.media_type,
                    artifact_root,
                    assistant_uuid,
                    index,
                    &mut created_artifacts,
                )?,
                AssistantContentPart::ToolResult(result) => externalize_tool_result(
                    &mut result.output,
                    artifact_root,
                    assistant_uuid,
                    index,
                    &mut created_artifacts,
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
    })();

    if result.is_err() {
        for path in created_artifacts.into_iter().rev() {
            let _ = std::fs::remove_file(&path);
        }
    }
    result
}

fn externalize_tool_result(
    output: &mut ToolResultContent,
    artifact_root: Option<&Path>,
    assistant_uuid: Uuid,
    part_index: usize,
    created_artifacts: &mut Vec<PathBuf>,
) -> Result<(), AssistantPayloadError> {
    let ToolResultContent::Content { value, .. } = output else {
        return Ok(());
    };
    for (content_index, content) in value.iter_mut().enumerate() {
        let ToolResultContentPart::FileData {
            data,
            media_type,
            provider_options,
            ..
        } = content
        else {
            continue;
        };
        if data.len() <= MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES {
            continue;
        }
        let root = artifact_root.ok_or_else(|| AssistantPayloadError {
            message: format!("generated {media_type} requires an artifact store"),
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|_| AssistantPayloadError {
                message: format!("provider returned invalid base64 for generated {media_type}"),
            })?;
        let filename = format!(
            "{assistant_uuid}-{part_index}-{content_index}.{}",
            media_extension(media_type)
        );
        let relative = PathBuf::from("assistant-media").join(filename);
        let path = artifact_write_path(root, &relative)?;
        let existed = path.exists();
        coco_utils_common::write_atomic(&path, &bytes).map_err(|error| AssistantPayloadError {
            message: format!(
                "failed to persist generated media {}: {error}",
                path.display()
            ),
        })?;
        if !existed {
            created_artifacts.push(path);
        }
        let references = HashMap::from([
            (
                COCO_ARTIFACT_REFERENCE.to_string(),
                relative.to_string_lossy().into_owned(),
            ),
            (COCO_ARTIFACT_MEDIA_TYPE.to_string(), media_type.clone()),
        ]);
        *content = ToolResultContentPart::FileReference {
            provider_reference: references,
            provider_options: provider_options.take(),
        };
    }
    Ok(())
}

fn rehydrate_tool_result(
    output: &mut ToolResultContent,
    artifact_root: Option<&Path>,
    budget: &mut RehydrationBudget,
) -> Result<(), AssistantPayloadError> {
    let ToolResultContent::Content { value, .. } = output else {
        return Ok(());
    };
    for content in value {
        let ToolResultContentPart::FileReference {
            provider_reference,
            provider_options,
        } = content
        else {
            continue;
        };
        let Some(relative) = provider_reference.get(COCO_ARTIFACT_REFERENCE) else {
            continue;
        };
        let media_type = provider_reference
            .get(COCO_ARTIFACT_MEDIA_TYPE)
            .ok_or_else(|| AssistantPayloadError {
                message: "generated media reference is missing its media type".into(),
            })?;
        let root = artifact_root.ok_or_else(|| AssistantPayloadError {
            message: "generated media reference requires an artifact store".into(),
        })?;
        let bytes = read_artifact(root, relative, budget)?;
        *content = ToolResultContentPart::FileData {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            media_type: media_type.clone(),
            filename: None,
            provider_options: provider_options.take(),
        };
    }
    Ok(())
}

fn rehydrate_file_data(
    data: &mut SharedV4FileData,
    artifact_root: Option<&Path>,
    budget: &mut RehydrationBudget,
) -> Result<(), AssistantPayloadError> {
    let SharedV4FileData::Reference { reference } = data else {
        return Ok(());
    };
    let Some(relative) = reference.get(COCO_ARTIFACT_REFERENCE) else {
        return Ok(());
    };
    let root = artifact_root.ok_or_else(|| AssistantPayloadError {
        message: "generated media reference requires an artifact store".into(),
    })?;
    let bytes = read_artifact(root, relative, budget)?;
    *data = SharedV4FileData::data_bytes(bytes);
    Ok(())
}

struct RehydrationBudget {
    encoded_bytes: usize,
    limit: usize,
}

impl Default for RehydrationBudget {
    fn default() -> Self {
        Self {
            encoded_bytes: 0,
            limit: MAX_ASSISTANT_TURN_BYTES,
        }
    }
}

impl RehydrationBudget {
    fn remaining_encoded_bytes(&self) -> usize {
        self.limit.saturating_sub(self.encoded_bytes)
    }

    fn charge_raw_bytes(&mut self, raw_bytes: usize) -> Result<(), AssistantPayloadError> {
        let encoded_bytes = raw_bytes.div_ceil(3).saturating_mul(4);
        let next = self.encoded_bytes.saturating_add(encoded_bytes);
        if next > self.limit {
            return Err(AssistantPayloadError {
                message: format!(
                    "rehydrated assistant media exceeds the {}-byte safety limit",
                    self.limit
                ),
            });
        }
        self.encoded_bytes = next;
        Ok(())
    }
}

fn read_artifact(
    root: &Path,
    relative: &str,
    budget: &mut RehydrationBudget,
) -> Result<Vec<u8>, AssistantPayloadError> {
    let relative = safe_relative_path(relative)?;
    let canonical_root = std::fs::canonicalize(root).map_err(|error| AssistantPayloadError {
        message: format!(
            "failed to resolve artifact store {}: {error}",
            root.display()
        ),
    })?;
    let requested = canonical_root.join(relative);
    let path = std::fs::canonicalize(&requested).map_err(|error| AssistantPayloadError {
        message: format!(
            "failed to resolve generated media {}: {error}",
            requested.display()
        ),
    })?;
    if !path.starts_with(&canonical_root) {
        return Err(AssistantPayloadError {
            message: format!("generated media reference escapes artifact store: {relative:?}"),
        });
    }
    let max_raw_bytes = budget
        .remaining_encoded_bytes()
        .saturating_div(4)
        .saturating_mul(3);
    let mut bytes = Vec::with_capacity(max_raw_bytes.min(64 * 1024));
    std::fs::File::open(&path)
        .and_then(|file| {
            file.take(max_raw_bytes.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| AssistantPayloadError {
            message: format!("failed to read generated media {}: {error}", path.display()),
        })?;
    budget.charge_raw_bytes(bytes.len())?;
    Ok(bytes)
}

fn safe_relative_path(value: &str) -> Result<&Path, AssistantPayloadError> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AssistantPayloadError {
            message: format!("invalid generated media reference: {value}"),
        });
    }
    Ok(path)
}

fn externalize_file_data(
    data: &mut SharedV4FileData,
    media_type: &str,
    artifact_root: Option<&Path>,
    assistant_uuid: Uuid,
    index: usize,
    created_artifacts: &mut Vec<PathBuf>,
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
    let root = artifact_root.ok_or_else(|| AssistantPayloadError {
        message: format!("generated {media_type} requires an artifact store"),
    })?;
    let bytes = raw.to_bytes().ok_or_else(|| AssistantPayloadError {
        message: format!("provider returned invalid base64 for generated {media_type}"),
    })?;
    let path = artifact_write_path(root, &relative)?;
    let existed = path.exists();
    coco_utils_common::write_atomic(&path, bytes).map_err(|error| AssistantPayloadError {
        message: format!(
            "failed to persist generated media {}: {error}",
            path.display()
        ),
    })?;
    if !existed {
        created_artifacts.push(path);
    }
    let reference = relative.to_string_lossy().into_owned();
    *data = SharedV4FileData::reference(HashMap::from([(
        COCO_ARTIFACT_REFERENCE.to_string(),
        reference,
    )]));
    Ok(())
}

fn artifact_write_path(root: &Path, relative: &Path) -> Result<PathBuf, AssistantPayloadError> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AssistantPayloadError {
            message: format!("invalid generated media path: {}", relative.display()),
        });
    }
    std::fs::create_dir_all(root).map_err(|error| AssistantPayloadError {
        message: format!(
            "failed to create artifact store {}: {error}",
            root.display()
        ),
    })?;
    let canonical_root = std::fs::canonicalize(root).map_err(|error| AssistantPayloadError {
        message: format!(
            "failed to resolve artifact store {}: {error}",
            root.display()
        ),
    })?;
    let relative_parent = relative.parent().ok_or_else(|| AssistantPayloadError {
        message: format!("generated media path has no parent: {}", relative.display()),
    })?;
    let parent = canonical_root.join(relative_parent);
    match std::fs::symlink_metadata(&parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(AssistantPayloadError {
                message: format!(
                    "generated media directory must not be a symbolic link: {}",
                    parent.display()
                ),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(AssistantPayloadError {
                message: format!(
                    "generated media directory is not a directory: {}",
                    parent.display()
                ),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&parent).map_err(|error| AssistantPayloadError {
                message: format!(
                    "failed to create generated media directory {}: {error}",
                    parent.display()
                ),
            })?;
        }
        Err(error) => {
            return Err(AssistantPayloadError {
                message: format!(
                    "failed to inspect generated media directory {}: {error}",
                    parent.display()
                ),
            });
        }
    }
    let canonical_parent =
        std::fs::canonicalize(&parent).map_err(|error| AssistantPayloadError {
            message: format!(
                "failed to resolve generated media directory {}: {error}",
                parent.display()
            ),
        })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(AssistantPayloadError {
            message: format!(
                "generated media directory escapes artifact store: {}",
                parent.display()
            ),
        });
    }
    let filename = relative.file_name().ok_or_else(|| AssistantPayloadError {
        message: format!(
            "generated media path has no filename: {}",
            relative.display()
        ),
    })?;
    Ok(canonical_parent.join(filename))
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
