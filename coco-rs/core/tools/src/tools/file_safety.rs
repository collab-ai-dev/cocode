use coco_tool_runtime::ToolError;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use std::io;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const MAX_SUGGESTION_ENTRIES: usize = 512;
const MAX_SUGGESTIONS: usize = 3;
const MIN_SUGGESTION_SIMILARITY: f64 = 0.75;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileTargetKind {
    Regular,
    Directory,
    Fifo,
    Socket,
    CharacterDevice,
    BlockDevice,
    SymbolicLink,
    Other,
}

impl FileTargetKind {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Regular => "a regular file",
            Self::Directory => "a directory",
            Self::Fifo => "a FIFO",
            Self::Socket => "a socket",
            Self::CharacterDevice => "a character device",
            Self::BlockDevice => "a block device",
            Self::SymbolicLink => "a symbolic link",
            Self::Other => "a non-regular file",
        }
    }
}

pub(crate) fn inspect_file_target(path: &Path) -> io::Result<FileTargetKind> {
    classify_file_type(std::fs::metadata(path)?.file_type())
}

pub(crate) fn inspect_mutation_target(path: &Path) -> io::Result<FileTargetKind> {
    classify_file_type(std::fs::symlink_metadata(path)?.file_type())
}

fn classify_file_type(file_type: std::fs::FileType) -> io::Result<FileTargetKind> {
    if file_type.is_file() {
        return Ok(FileTargetKind::Regular);
    }
    if file_type.is_dir() {
        return Ok(FileTargetKind::Directory);
    }
    if file_type.is_symlink() {
        return Ok(FileTargetKind::SymbolicLink);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        if file_type.is_fifo() {
            return Ok(FileTargetKind::Fifo);
        }
        if file_type.is_socket() {
            return Ok(FileTargetKind::Socket);
        }
        if file_type.is_char_device() {
            return Ok(FileTargetKind::CharacterDevice);
        }
        if file_type.is_block_device() {
            return Ok(FileTargetKind::BlockDevice);
        }
    }

    Ok(FileTargetKind::Other)
}

pub(crate) async fn missing_path_message(
    path: &Path,
    ctx: &coco_tool_runtime::ToolUseContext,
) -> String {
    let mut message = format!("File not found: {}", path.display());
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return message;
    };
    let raw_parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = if raw_parent.is_absolute() {
        raw_parent.to_path_buf()
    } else {
        ctx.cwd_anchor()
            .await
            .unwrap_or_else(|| PathBuf::from("."))
            .join(raw_parent)
    };
    if matches!(
        super::read_permissions::check_background_read_permission_with_sandbox(&parent, ctx).await,
        coco_types::ToolCheckResult::Ask { .. } | coco_types::ToolCheckResult::Deny { .. }
    ) {
        return message;
    }
    let requested = comparable_name(file_name);
    let Ok(nearby) =
        tokio::task::spawn_blocking(move || scan_suggestions(&parent, &requested)).await
    else {
        return message;
    };

    let mut candidates = Vec::with_capacity(MAX_SUGGESTIONS);
    for (exact, score, candidate_path) in nearby {
        if matches!(
            super::read_permissions::check_background_read_permission_with_sandbox(
                &candidate_path,
                ctx,
            )
            .await,
            coco_types::ToolCheckResult::Ask { .. } | coco_types::ToolCheckResult::Deny { .. }
        ) {
            continue;
        }
        candidates.push((exact, score, candidate_path));
        if candidates.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    if candidates.is_empty() {
        return message;
    }

    let label = if candidates[0].0 {
        " Unicode-equivalent path exists:"
    } else {
        " Did you mean:"
    };
    message.push_str(label);
    for (_, _, candidate) in candidates {
        message.push_str(&format!("\n- {}", candidate.display()));
    }
    message
}

fn scan_suggestions(parent: &Path, requested: &str) -> Vec<(bool, f64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let entries: Vec<_> = entries
        .filter_map(Result::ok)
        .take(MAX_SUGGESTION_ENTRIES + 1)
        .collect();
    if entries.len() > MAX_SUGGESTION_ENTRIES {
        return Vec::new();
    }

    let mut nearby = Vec::new();
    for entry in entries {
        let candidate_path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let comparable = comparable_name(name);
        let exact = comparable == requested;
        let score = strsim::normalized_levenshtein(requested, &comparable);
        if !exact && score < MIN_SUGGESTION_SIMILARITY {
            continue;
        }
        if !matches!(
            inspect_file_target(&candidate_path),
            Ok(FileTargetKind::Regular)
        ) {
            continue;
        }
        nearby.push((exact, score, candidate_path));
    }
    nearby.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.total_cmp(&left.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    nearby
}

/// Success marker serialized as the literal JSON boolean `true`.
///
/// Deserialization rejects `false`, making an unverified success state
/// unrepresentable in typed tool outputs.
#[derive(Debug, Clone, Copy)]
#[must_use]
pub struct VerifiedWrite(());

impl Serialize for VerifiedWrite {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for VerifiedWrite {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self(()))
        } else {
            Err(serde::de::Error::custom("verified write must be true"))
        }
    }
}

pub(crate) fn commit_file(path: &Path, expected: &[u8]) -> Result<VerifiedWrite, ToolError> {
    let commit = || coco_utils_common::replace_regular_atomic(path, expected);
    let result = match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(commit)
        }
        _ => commit(),
    };
    result
        .map(|_| VerifiedWrite(()))
        .map_err(|error| ToolError::ExecutionFailed {
            message: format!(
                "failed to atomically write and verify {}: {error}",
                path.display()
            ),
            display_data: None,
            source: None,
        })
}

fn comparable_name(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            '\u{00a0}' | '\u{202f}' => ' ',
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            other => other,
        })
        .collect::<String>()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
#[path = "file_safety.test.rs"]
mod tests;
