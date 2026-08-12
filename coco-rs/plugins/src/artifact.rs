//! Structural inspection for staged plugin artifacts.

use std::io::Read;
use std::path::Path;

use sha2::Digest;
use sha2::Sha256;

const MAX_ARTIFACT_FILES: i64 = 10_000;
const MAX_ARTIFACT_ENTRIES: i64 = 20_000;
const MAX_ARTIFACT_BYTES: i64 = 128 * 1024 * 1024;

/// Trusted facts computed over a fully materialized plugin tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInspection {
    pub tree_sha256: String,
    pub file_count: i64,
    pub total_bytes: i64,
}

/// Inspect a staged artifact without following links.
///
/// The digest covers normalized relative paths, file lengths, and file bytes in
/// sorted path order. Symlinks and special files are rejected: plugin loading
/// must never reach outside the staged tree or depend on device/FIFO behavior.
pub fn inspect_artifact(root: &Path) -> crate::Result<ArtifactInspection> {
    let root_type = std::fs::symlink_metadata(root)?.file_type();
    if !root_type.is_dir() {
        return Err(crate::PluginError::generic(
            "artifact",
            format!("plugin artifact is not a directory: {}", root.display()),
        ));
    }

    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    let mut total_bytes = 0i64;
    for (relative, path, length) in &files {
        total_bytes = total_bytes.checked_add(*length).ok_or_else(|| {
            crate::PluginError::generic("artifact", "plugin artifact byte count overflow")
        })?;
        if total_bytes > MAX_ARTIFACT_BYTES {
            return Err(crate::PluginError::generic(
                "artifact",
                format!("plugin artifact exceeds {MAX_ARTIFACT_BYTES} bytes"),
            ));
        }

        hasher.update((relative.len() as u64).to_le_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((*length as u64).to_le_bytes());

        let mut file = coco_utils_common::open_regular(path)?;
        if file.metadata()?.len() != *length as u64 {
            return Err(crate::PluginError::generic(
                "artifact",
                format!(
                    "plugin artifact changed during inspection: {}",
                    path.display()
                ),
            ));
        }
        let mut buffer = [0u8; 64 * 1024];
        let mut bytes_read = 0u64;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            bytes_read = bytes_read.saturating_add(read as u64);
            hasher.update(&buffer[..read]);
        }
        if bytes_read != *length as u64 {
            return Err(crate::PluginError::generic(
                "artifact",
                format!(
                    "plugin artifact changed during inspection: {}",
                    path.display()
                ),
            ));
        }
    }

    Ok(ArtifactInspection {
        tree_sha256: hex::encode(hasher.finalize()),
        file_count: files.len() as i64,
        total_bytes,
    })
}

fn collect_files(
    root: &Path,
    initial_directory: &Path,
    files: &mut Vec<(String, std::path::PathBuf, i64)>,
) -> crate::Result<()> {
    let mut directories = vec![initial_directory.to_path_buf()];
    let mut entry_count = 0i64;
    while let Some(directory) = directories.pop() {
        let entries = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        for entry in entries {
            entry_count = entry_count.checked_add(1).ok_or_else(|| {
                crate::PluginError::generic("artifact", "plugin artifact entry count overflow")
            })?;
            if entry_count > MAX_ARTIFACT_ENTRIES {
                return Err(crate::PluginError::generic(
                    "artifact",
                    format!("plugin artifact exceeds {MAX_ARTIFACT_ENTRIES} directory entries"),
                ));
            }

            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(crate::PluginError::generic(
                    "artifact",
                    format!("plugin artifact contains a symlink: {}", path.display()),
                ));
            }
            if file_type.is_dir() {
                directories.push(path);
                continue;
            }
            if !file_type.is_file() {
                return Err(crate::PluginError::generic(
                    "artifact",
                    format!(
                        "plugin artifact contains a special file: {}",
                        path.display()
                    ),
                ));
            }

            let relative = path.strip_prefix(root).map_err(|error| {
                crate::PluginError::generic("artifact", format!("invalid artifact path: {error}"))
            })?;
            let relative = relative.to_str().ok_or_else(|| {
                crate::PluginError::generic(
                    "artifact",
                    format!("plugin artifact path is not UTF-8: {}", path.display()),
                )
            })?;
            let normalized = relative.replace(std::path::MAIN_SEPARATOR, "/");
            let length = i64::try_from(metadata.len()).map_err(|_| {
                crate::PluginError::generic(
                    "artifact",
                    format!("plugin artifact file is too large: {}", path.display()),
                )
            })?;
            files.push((normalized, path, length));
            if files.len() as i64 > MAX_ARTIFACT_FILES {
                return Err(crate::PluginError::generic(
                    "artifact",
                    format!("plugin artifact exceeds {MAX_ARTIFACT_FILES} files"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "artifact.test.rs"]
mod tests;
