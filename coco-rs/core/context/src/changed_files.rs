//! Changed file detection via FileReadState mtime comparison.
//!
//! Iterates all files in readFileState, compares cached mtime vs disk
//! mtime, creates diff attachments for externally modified files.

use crate::attachment::Attachment;
use crate::attachment::FileAttachment;
use crate::file_read_state::FileReadEntry;
use crate::file_read_state::FileReadRange;
use crate::file_read_state::FileReadState;
use crate::file_read_state::file_mtime_ms;

/// Detect files that changed on disk since they were last read.
///
/// Skips line-range reads since they can't be reliably diffed.
/// Updates the cache with new content after detecting changes.
pub async fn detect_changed_files(file_read_state: &mut FileReadState) -> Vec<Attachment> {
    let mut changed = Vec::new();

    let paths_to_check: Vec<(std::path::PathBuf, i64)> = file_read_state
        .iter_entries()
        .filter(|(_, entry)| entry.range == FileReadRange::Full)
        .map(|(path, entry)| (path.to_path_buf(), entry.mtime_ms))
        .collect();

    for (path, cached_mtime) in paths_to_check {
        let disk_mtime = match file_mtime_ms(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };

        if disk_mtime <= cached_mtime {
            continue;
        }

        let new_content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let filename = path.to_string_lossy().into_owned();

        // Update cache first so we can move new_content into the attachment.
        file_read_state.set(
            path,
            FileReadEntry::full_real(new_content.clone(), disk_mtime),
        );

        changed.push(Attachment::File(FileAttachment {
            display_path: filename.clone(),
            filename,
            content: new_content,
            truncated: false,
            offset: None,
            limit: None,
        }));
    }

    changed
}

#[cfg(test)]
#[path = "changed_files.test.rs"]
mod tests;
