//! Changed file detection via FileReadState mtime comparison.
//!
//! Creates diff observations for files changed since the model last saw them.
//!
//! This module does not enforce permissions or read files from disk. App/tool
//! layers perform those steps, then pass loaded content here to build
//! FileReadState observations and reminder attachments.

use crate::attachment::Attachment;
use crate::attachment::EditedImageFileAttachment;
use crate::attachment::EditedTextFileAttachment;
use crate::file_read_state::FileReadEntry;
use crate::file_read_state::FileReadRange;
use crate::file_read_state::FileReadState;
use std::path::PathBuf;

const DIFF_CONTEXT_RADIUS: usize = 8;
pub const DIFF_INPUT_MAX_BYTES: usize = 1024 * 1024;
const DIFF_SNIPPET_MAX_BYTES: usize = 8192;

#[derive(Debug, Clone)]
pub struct ChangedFileCandidate {
    pub path: PathBuf,
    pub cached_entry: FileReadEntry,
    pub cached_mtime_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ChangedFileUpdate {
    pub path: PathBuf,
    pub entry: FileReadEntry,
    pub preserve_read_origin: bool,
    pub attachment: Option<Attachment>,
}

#[derive(Debug, Clone)]
pub enum ChangedFileObservation {
    Updated(ChangedFileUpdate),
    Deleted { path: PathBuf },
}

#[derive(Debug, Clone)]
pub enum ChangedFileLoadedContent {
    Text { content: String, mtime_ms: i64 },
    Image { mtime_ms: i64 },
    Unsupported { mtime_ms: i64 },
}

/// Snapshot full-file entries that can be checked for external changes.
pub fn changed_file_candidates(file_read_state: &FileReadState) -> Vec<ChangedFileCandidate> {
    file_read_state
        .iter_entries()
        .filter(|(_, entry)| entry.range == FileReadRange::Full)
        .map(|(path, entry)| ChangedFileCandidate {
            path: path.to_path_buf(),
            cached_entry: entry.clone(),
            cached_mtime_ms: entry.mtime_ms,
        })
        .collect()
}

pub fn changed_file_observation_from_loaded(
    candidate: ChangedFileCandidate,
    loaded: ChangedFileLoadedContent,
) -> ChangedFileObservation {
    let filename = candidate.path.to_string_lossy().into_owned();
    match loaded {
        ChangedFileLoadedContent::Image { mtime_ms } => {
            let attachment = Some(Attachment::EditedImageFile(EditedImageFileAttachment {
                display_path: filename.clone(),
                filename,
            }));
            ChangedFileObservation::Updated(ChangedFileUpdate {
                path: candidate.path,
                entry: FileReadEntry::observed_for_diff(String::new(), mtime_ms),
                preserve_read_origin: false,
                attachment,
            })
        }
        ChangedFileLoadedContent::Unsupported { mtime_ms } => {
            ChangedFileObservation::Updated(ChangedFileUpdate {
                path: candidate.path,
                entry: FileReadEntry::observed_for_diff(candidate.cached_entry.content, mtime_ms),
                preserve_read_origin: false,
                attachment: None,
            })
        }
        ChangedFileLoadedContent::Text { content, mtime_ms } => {
            if content == candidate.cached_entry.content {
                let mut entry = candidate.cached_entry;
                entry.content = content;
                entry.mtime_ms = mtime_ms;
                return ChangedFileObservation::Updated(ChangedFileUpdate {
                    path: candidate.path,
                    entry,
                    preserve_read_origin: true,
                    attachment: None,
                });
            }
            let attachment =
                diff_snippet_with_line_numbers(&candidate.cached_entry.content, &content).map(
                    |snippet| {
                        Attachment::EditedTextFile(EditedTextFileAttachment {
                            display_path: filename.clone(),
                            filename,
                            snippet,
                        })
                    },
                );

            ChangedFileObservation::Updated(ChangedFileUpdate {
                path: candidate.path,
                entry: FileReadEntry::observed_for_diff(content, mtime_ms),
                preserve_read_origin: false,
                attachment,
            })
        }
    }
}

pub fn deleted_changed_file_observation(path: PathBuf) -> ChangedFileObservation {
    ChangedFileObservation::Deleted { path }
}

/// Apply detected observations to FileReadState.
///
/// Changed content is not edit/write evidence: the model saw only a
/// snippet or silent marker. Mtime-only refreshes preserve prior evidence.
pub fn apply_changed_file_observations(
    file_read_state: &mut FileReadState,
    observations: &[ChangedFileObservation],
) {
    for observation in observations {
        match observation {
            ChangedFileObservation::Updated(update) if update.preserve_read_origin => {
                file_read_state
                    .set_preserving_read_origin(update.path.clone(), update.entry.clone());
            }
            ChangedFileObservation::Updated(update) => {
                file_read_state.set(update.path.clone(), update.entry.clone());
            }
            ChangedFileObservation::Deleted { path } => {
                file_read_state.invalidate(path);
            }
        }
    }
}

fn changed_file_attachments(observations: &[ChangedFileObservation]) -> Vec<Attachment> {
    observations
        .iter()
        .filter_map(|observation| match observation {
            ChangedFileObservation::Updated(update) => update.attachment.clone(),
            ChangedFileObservation::Deleted { .. } => None,
        })
        .collect()
}

/// Collect model-visible attachments from detected observations.
pub fn attachments_from_changed_file_observations(
    observations: &[ChangedFileObservation],
) -> Vec<Attachment> {
    changed_file_attachments(observations)
}

fn diff_snippet_with_line_numbers(old_content: &str, new_content: &str) -> Option<String> {
    let input_bytes = old_content.len().saturating_add(new_content.len());
    if input_bytes > DIFF_INPUT_MAX_BYTES {
        tracing::warn!(
            input_bytes,
            max_bytes = DIFF_INPUT_MAX_BYTES,
            old_bytes = old_content.len(),
            new_bytes = new_content.len(),
            "skipping changed-file diff snippet because input is too large"
        );
        return None;
    }

    let diff = similar::TextDiff::from_lines(old_content, new_content);
    let grouped_ops = diff.grouped_ops(DIFF_CONTEXT_RADIUS);
    if grouped_ops.is_empty() {
        return None;
    }

    let sections = grouped_ops
        .into_iter()
        .filter_map(|group| {
            let start_line = group
                .iter()
                .flat_map(|op| diff.iter_changes(op))
                .find_map(|change| change.old_index().or_else(|| change.new_index()))
                .map(|idx| idx + 1)
                .unwrap_or(1);
            let lines = group
                .iter()
                .flat_map(|op| diff.iter_changes(op))
                .filter(|change| change.tag() != similar::ChangeTag::Delete)
                .enumerate()
                .map(|(idx, change)| {
                    let line_number = start_line + idx;
                    let line = change.value().trim_end_matches(['\r', '\n']);
                    format!("{line_number:>6}\t{line}")
                })
                .collect::<Vec<_>>();
            (!lines.is_empty()).then(|| lines.join("\n"))
        })
        .collect::<Vec<_>>();

    if sections.is_empty() {
        return None;
    }

    let full = sections.join("\n...\n");
    Some(truncate_diff_snippet(full))
}

fn truncate_diff_snippet(full: String) -> String {
    if full.len() <= DIFF_SNIPPET_MAX_BYTES {
        return full;
    }

    let cutoff = full[..full.floor_char_boundary(DIFF_SNIPPET_MAX_BYTES)]
        .rfind('\n')
        .unwrap_or_else(|| full.floor_char_boundary(DIFF_SNIPPET_MAX_BYTES));
    let kept = &full[..cutoff];
    let remaining = full[cutoff..].lines().count();
    format!("{kept}\n\n... [{remaining} lines truncated] ...")
}

#[cfg(test)]
#[path = "changed_files.test.rs"]
mod tests;
