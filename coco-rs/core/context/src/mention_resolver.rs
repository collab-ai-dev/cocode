//! Mention → Attachment resolution with FileReadState deduplication.
//!
//! Resolves @-mentioned files into Attachment objects, checking
//! `readFileState` for dedup (returns `AlreadyReadFileAttachment` if
//! unchanged).

use std::{path::Path, sync::Arc, time::Duration};

use tokio::sync::RwLock;

use crate::attachment::AlreadyReadFileAttachment;
use crate::attachment::Attachment;
use crate::attachment::AttachmentBudget;
use crate::attachment::DirectoryAttachment;
use crate::attachment::FileReadOptions;
use crate::attachment::generate_file_attachment;
use crate::file_read_state::FileReadEntry;
use crate::file_read_state::FileReadRange;
use crate::file_read_state::FileReadState;
use crate::file_read_state::file_mtime_ms;
use crate::user_input::Mention;
use crate::user_input::MentionType;

/// Options for mention resolution.
pub struct MentionResolveOptions<'a> {
    /// Current working directory for relative path expansion.
    pub cwd: &'a Path,
    /// Maximum directory entries to list.
    pub max_dir_entries: i32,
}

impl Default for MentionResolveOptions<'_> {
    fn default() -> Self {
        Self {
            cwd: Path::new("."),
            max_dir_entries: 1000,
        }
    }
}

/// Resolve a list of mentions into attachments.
/// Checks `file_read_state` for dedup: if a file is cached and its mtime
/// hasn't changed, returns `AlreadyReadFile` instead of re-reading.
/// After reading a new file, updates `file_read_state` with its content and mtime.
pub async fn resolve_mentions(
    mentions: &[Mention],
    file_read_state: &Arc<RwLock<FileReadState>>,
    options: &MentionResolveOptions<'_>,
) -> Vec<Attachment> {
    let mut attachments = Vec::new();

    for mention in mentions.iter().take(MAX_MENTIONS_PER_TURN) {
        match &mention.mention_type {
            MentionType::FilePath => {
                if let Some(att) = resolve_file_mention(mention, file_read_state, options).await {
                    attachments.push(att);
                }
            }
            MentionType::Agent => {
                attachments.push(Attachment::AgentMention(
                    crate::attachment::AgentMentionAttachment {
                        agent_type: mention.text.clone(),
                    },
                ));
            }
            MentionType::McpResource { .. } => {
                // MCP resource fetch is wired through `services/mcp` at the
                // call site (the resolver doesn't depend on MCP). Once the
                // caller has the client handle it can post-process this
                // mention; here we just preserve the parse result so the
                // caller can iterate `mentions` separately when needed.
            }
            MentionType::Url | MentionType::Symbol => {
                // URL and symbol mentions not resolved to attachments yet.
            }
        }
    }

    AttachmentBudget::new(MAX_MENTION_TOKENS_PER_TURN).filter_within_budget(attachments)
}

const MAX_MENTIONS_PER_TURN: usize = 32;
const MAX_MENTION_TOKENS_PER_TURN: i64 = 16_000;
const MAX_TEXT_FILE_BYTES: u64 = 256 * 1024;
const MAX_BINARY_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MENTION_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Resolve a single file mention to an attachment.
async fn resolve_file_mention(
    mention: &Mention,
    file_read_state: &Arc<RwLock<FileReadState>>,
    options: &MentionResolveOptions<'_>,
) -> Option<Attachment> {
    let raw_path = Path::new(&mention.text);
    let abs_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        options.cwd.join(raw_path)
    };

    let display_path = abs_path
        .strip_prefix(options.cwd)
        .unwrap_or(&abs_path)
        .to_string_lossy()
        .into_owned();

    let metadata = tokio::fs::metadata(&abs_path).await.ok()?;
    if !metadata.is_file() && !metadata.is_dir() {
        return None;
    }

    // Directory handling
    if metadata.is_dir() {
        let path = abs_path.clone();
        let max_entries = options.max_dir_entries;
        return tokio::time::timeout(
            MENTION_READ_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                resolve_directory(&path, &display_path, max_entries)
            }),
        )
        .await
        .ok()?
        .ok()
        .flatten();
    }

    let max_bytes = if is_binary_mention(&abs_path) {
        MAX_BINARY_FILE_BYTES
    } else {
        MAX_TEXT_FILE_BYTES
    };
    if metadata.len() > max_bytes {
        return None;
    }

    // FileReadState is keyed by CANONICAL path — matching the Read primer
    // (`coco_tools::record_file_read`) and the Edit/Write read-before-edit
    // guards, which canonicalize. Caching an @mention under a lexical
    // `cwd.join` key would hide it from those guards on any symlinked path
    // (e.g. macOS `/tmp` → `/private/tmp`), triggering a spurious
    // "has not been read yet" rejection on a subsequent edit.
    let key_path = tokio::fs::canonicalize(&abs_path)
        .await
        .unwrap_or_else(|_| abs_path.clone());

    // Dedup check: if file is in FileReadState and mtime hasn't changed,
    // return AlreadyReadFileAttachment.
    let cached = file_read_state.read().await.peek(&key_path).cloned();
    if let Some(entry) = cached
        && let Ok(disk_mtime) = file_mtime_ms(&key_path).await
        && entry.mtime_ms == disk_mtime
    {
        return Some(Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
            filename: abs_path.to_string_lossy().into_owned(),
            display_path,
        }));
    }

    // Read the file via the existing attachment generator.
    let read_options = FileReadOptions {
        offset: mention.line_start,
        limit: mention.line_end.map(|end| {
            // Convert line range to limit: #L10-20 → offset=10, limit=11
            end - mention.line_start.unwrap_or(1) + 1
        }),
        ..Default::default()
    };

    let path = abs_path.clone();
    let cwd = options.cwd.to_path_buf();
    let blocking_options = read_options.clone();
    let attachment = tokio::time::timeout(
        MENTION_READ_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            generate_file_attachment(&path, &cwd, &blocking_options)
        }),
    )
    .await
    .ok()?
    .ok()
    .flatten()?;

    // Update FileReadState with new content and mtime, keyed by canonical path.
    update_file_read_state(file_read_state, &key_path, &attachment, &read_options).await;

    Some(attachment)
}

/// Update FileReadState after resolving a mention.
async fn update_file_read_state(
    state: &Arc<RwLock<FileReadState>>,
    abs_path: &Path,
    attachment: &Attachment,
    options: &FileReadOptions,
) {
    let (content, truncated) = match attachment {
        Attachment::File(f) => (f.content.clone(), f.truncated),
        // Images and PDFs don't populate text content in FileReadState.
        _ => return,
    };

    if let Ok(mtime) = file_mtime_ms(abs_path).await {
        let range = match (options.offset, options.limit) {
            (None, None) => FileReadRange::Full,
            (offset, Some(limit)) => FileReadRange::Lines { offset, limit },
            (offset, None) => FileReadRange::Lines {
                offset,
                limit: i32::MAX,
            },
        };
        let entry = if truncated || range != FileReadRange::Full {
            FileReadEntry::injected_partial(content, mtime, range)
        } else {
            FileReadEntry::full_real(content, mtime)
        };
        state.write().await.set(abs_path.to_path_buf(), entry);
    }
}

fn is_binary_mention(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf")
    )
}

/// Resolve a directory mention: list entries up to `max_entries`.
///: bare entry names (no trailing `/`),
/// and when the directory exceeds the cap a trailing `… and N more entries`
/// line carrying the exact overflow count.
fn resolve_directory(path: &Path, display_path: &str, max_entries: i32) -> Option<Attachment> {
    let max_entries = max_entries.max(0) as usize;
    let names: Vec<String> = std::fs::read_dir(path)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();

    let total = names.len();
    let mut lines: Vec<String> = names.into_iter().take(max_entries).collect();
    if total > max_entries {
        let remaining = total - max_entries;
        lines.push(format!("… and {remaining} more entries"));
    }

    Some(Attachment::Directory(DirectoryAttachment {
        path: path.to_string_lossy().into_owned(),
        content: lines.join("\n"),
        display_path: display_path.to_string(),
    }))
}

#[cfg(test)]
#[path = "mention_resolver.test.rs"]
mod tests;
