//! Shared `@`-mention resolution + turn-message construction used by every
//! entry path (TUI, headless, SDK).
//!
//! Resolves `@path` mentions to file content, renders them as synthetic
//! `Read`-tool narration wrapped in `<system-reminder>`, and tracks the
//! `FileReadState` dedup cache so subsequent turns return
//! `Attachment::AlreadyReadFile` instead of re-loading.
//!
//! The renderer bodies live here (not in `coco-messages` or `coco-context`)
//! because they stitch together types from three crates — `coco_context::Attachment`,
//! `coco_messages::Message`, and `coco_types::ToolName` — and CLI is the
//! one layer that already depends on all three.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;
use uuid::Uuid;

use coco_context::Attachment;
use coco_context::FileReadState;
use coco_context::MentionResolveOptions;
use coco_llm_types::FilePart;
use coco_llm_types::UserContentPart;
use coco_messages::Message;
use coco_tui::ImageData;

/// Output of the per-turn user-input resolution pipeline.
///
/// Field order is the injection order: the user message first
/// (carrying the prompt + any clipboard images), then per-attachment
/// system-reminder messages with file/image/dir content, then any
/// edited-file notifications. [`build_messages_for_turn`] concatenates
/// them in that order.
pub struct ResolvedTurnInputs {
    /// The user-role message carrying the prompt text (+ inline images
    /// if `images` was non-empty).
    pub user_message: Message,
    /// System-reminder messages for resolved `@`-mentioned files /
    /// images / directories. Each attachment expands into two messages
    /// (synthetic `tool_use` text + `tool_result`), individually wrapped
    /// in `<system-reminder>` (image blocks pass through unwrapped).
    /// See [`attachment_to_messages`].
    pub attachment_messages: Vec<Message>,
    /// One system-reminder note per file detected as modified externally
    /// since the last turn.
    pub changed_file_messages: Vec<Message>,
    /// Absolute paths of files this turn either loaded or recognized as
    /// already-loaded. Engine consumers thread this into
    /// `engine.note_mentioned_paths` for post-compact restoration.
    pub mentioned_paths: Vec<std::path::PathBuf>,
}

/// Maximum number of directory entries listed when resolving a directory
/// mention. Mirrors the value used by the TUI submit path.
const MAX_DIR_ENTRIES: i32 = 1000;

/// Run the full mention-resolution pipeline for a user turn.
///
/// Steps:
/// 1. `coco_context::process_user_input` — extract `@` mentions.
/// 2. `coco_context::resolve_mentions` — load file content / resolve
///    directory listings, with `FileReadState` dedup.
/// 3. `coco_context::detect_changed_files` — find files modified since
///    last seen.
/// 4. Build the user message (text + optional image parts) with
///    `user_uuid`, then per-attachment reminder messages.
pub async fn resolve_turn_inputs(
    content: &str,
    images: &[ImageData],
    cwd: &Path,
    user_uuid: Uuid,
    file_read_state: &Arc<RwLock<FileReadState>>,
) -> ResolvedTurnInputs {
    let processed = coco_context::process_user_input(content);

    let (file_attachments, changed_file_attachments) = {
        let mut frs = file_read_state.write().await;
        let opts = MentionResolveOptions {
            cwd,
            max_dir_entries: MAX_DIR_ENTRIES,
        };
        let file_attachments =
            coco_context::resolve_mentions(&processed.mentions, &mut frs, &opts).await;
        let changed_file_attachments = coco_context::detect_changed_files(&mut frs).await;
        (file_attachments, changed_file_attachments)
    };

    let mentioned_paths: Vec<std::path::PathBuf> = file_attachments
        .iter()
        .filter_map(|att| match att {
            Attachment::File(f) => Some(std::path::PathBuf::from(&f.filename)),
            Attachment::AlreadyReadFile(f) => Some(std::path::PathBuf::from(&f.filename)),
            _ => None,
        })
        .collect();

    let user_message = build_user_message(user_uuid, content, images);

    let mut attachment_messages: Vec<Message> = file_attachments
        .iter()
        .flat_map(attachment_to_messages)
        .collect();
    // Display-only summary first so the transcript shows a compact
    // `⎿ Read <path> (N lines)` / `⎿ Listed directory <path>/` row directly
    // under the user prompt. Carries no API tokens (the model-visible content
    // rides the `<system-reminder>` messages above); never reaches the model.
    if let Some(summary) = mention_summary_message(&file_attachments) {
        attachment_messages.insert(0, summary);
    }

    let changed_file_messages: Vec<Message> = changed_file_attachments
        .iter()
        .filter_map(changed_file_to_message)
        .collect();

    ResolvedTurnInputs {
        user_message,
        attachment_messages,
        changed_file_messages,
        mentioned_paths,
    }
}

/// Convenience wrapper for non-TUI callers that have no clipboard image
/// state and no externally-minted UUID: synthesizes a fresh `Uuid` and
/// passes an empty `images` slice.
pub async fn resolve_turn_inputs_text_only(
    content: &str,
    cwd: &Path,
    file_read_state: &Arc<RwLock<FileReadState>>,
) -> ResolvedTurnInputs {
    resolve_turn_inputs(content, &[], cwd, Uuid::new_v4(), file_read_state).await
}

/// Concatenate the inputs into a `Vec<Message>` in order:
/// `user_message` → file/image/dir reminders → changed-file notes.
///
/// Engine callers pass the result to [`engine.run_with_messages`].
pub fn build_messages_for_turn(inputs: &ResolvedTurnInputs) -> Vec<Message> {
    let mut messages = Vec::with_capacity(
        1 + inputs.attachment_messages.len() + inputs.changed_file_messages.len(),
    );
    messages.push(inputs.user_message.clone());
    messages.extend(inputs.attachment_messages.iter().cloned());
    messages.extend(inputs.changed_file_messages.iter().cloned());
    messages
}

/// Convert a resolved `@`-mention attachment into the model-visible
/// system-reminder messages.
///
/// Produces *two* messages per attachment: a synthetic `tool_use`
/// narration + `tool_result` wrapped in `<system-reminder>`. The image
/// branch keeps the image block unwrapped because `<system-reminder>`
/// only wraps text blocks.
///
/// Returning a `Vec` (vs the previous `Option`) lets us emit the
/// exact two-message shape; callers `flat_map` the results.
pub fn attachment_to_messages(att: &Attachment) -> Vec<Message> {
    let read_tool = coco_types::ToolName::Read.as_str();
    let bash_tool = coco_types::ToolName::Bash.as_str();

    match att {
        Attachment::File(f) => {
            let call = format!(
                "Called the {read_tool} tool with the following input: {{\"file_path\":\"{}\"}}",
                f.filename
            );
            let result = format!("Result of calling the {read_tool} tool:\n{}", f.content);
            vec![
                coco_messages::wrapping::create_system_reminder_message(&call),
                coco_messages::wrapping::create_system_reminder_message(&result),
            ]
        }
        Attachment::Image(img) => {
            let Some(b64) = img.base64_data.as_ref() else {
                return Vec::new();
            };
            let call = format!(
                "Called the {read_tool} tool with the following input: {{\"file_path\":\"{}\"}}",
                img.filename
            );
            // First message: text-only system-reminder with the synthetic
            // tool-use narration. Second message: the image block by itself
            // — unwrapped, because `<system-reminder>` only wraps text blocks.
            vec![
                coco_messages::wrapping::create_system_reminder_message(&call),
                coco_messages::create_user_message_with_parts(vec![UserContentPart::File(
                    FilePart::image_base64(b64, &img.media_type),
                )]),
            ]
        }
        Attachment::Directory(d) => {
            // Mirror TS `messages.ts` directory case: `ls <quoted-abs-path>`
            // with the absolute path (on-demand shell-quoting in the command —
            // bare when no metachars — and the bare path in the description).
            let quoted_path = coco_shell::shell_quoting::quote_posix(&[d.path.as_str()]);
            let call = format!(
                "Called the {bash_tool} tool with the following input: \
                 {{\"command\":\"ls {quoted_path}\",\"description\":\"Lists files in {}\"}}",
                d.path
            );
            let result = format!("Result of calling the {bash_tool} tool:\n{}", d.content);
            vec![
                coco_messages::wrapping::create_system_reminder_message(&call),
                coco_messages::wrapping::create_system_reminder_message(&result),
            ]
        }
        Attachment::AlreadyReadFile(_) | Attachment::AgentMention(_) => Vec::new(),
        _ => Vec::new(),
    }
}

/// Build the display-only `@`-mention summary attachment — one compact row
/// per resolved file / directory / image / PDF.
///
/// Returns `None` when nothing displayable resolved. The model-visible content
/// is injected separately by [`attachment_to_messages`]; this attachment has a
/// `Unit` body and is dropped from the API request, existing purely so the
/// transcript can render a tidy summary in place of the raw
/// `@-mentioned files` system-reminder.
fn mention_summary_message(atts: &[Attachment]) -> Option<Message> {
    use coco_messages::MentionItemKind;
    use coco_messages::MentionSummaryItem;
    use coco_messages::MentionSummaryPayload;

    let items: Vec<MentionSummaryItem> = atts
        .iter()
        .filter_map(|att| match att {
            Attachment::File(f) => Some(MentionSummaryItem {
                display_path: f.display_path.clone(),
                kind: MentionItemKind::File,
                count: Some(f.content.lines().count() as i32),
                truncated: f.truncated,
            }),
            Attachment::AlreadyReadFile(f) => Some(MentionSummaryItem {
                display_path: f.display_path.clone(),
                kind: MentionItemKind::AlreadyRead,
                count: None,
                truncated: false,
            }),
            Attachment::Directory(d) => Some(MentionSummaryItem {
                display_path: d.display_path.clone(),
                kind: MentionItemKind::Directory,
                count: None,
                truncated: false,
            }),
            Attachment::Image(img) => Some(MentionSummaryItem {
                display_path: img.display_path.clone(),
                kind: MentionItemKind::Image,
                count: None,
                truncated: false,
            }),
            Attachment::PdfReference(p) => Some(MentionSummaryItem {
                display_path: p.display_path.clone(),
                kind: MentionItemKind::Pdf,
                count: Some(p.page_count),
                truncated: false,
            }),
            _ => None,
        })
        .collect();

    if items.is_empty() {
        return None;
    }
    Some(Message::Attachment(
        coco_messages::AttachmentMessage::mention_summary(MentionSummaryPayload { items }),
    ))
}

/// Convert a `detect_changed_files` attachment into the externally-modified
/// notification message.
pub fn changed_file_to_message(att: &Attachment) -> Option<Message> {
    match att {
        Attachment::File(f) => {
            let text = format!(
                "Note: {} was modified, either by the user or by a linter. \
                 This change was intentional, so make sure to take it into \
                 account as you proceed (ie. don't revert it unless the user \
                 asks you to). Don't tell the user this, since they are already \
                 aware. Here are the relevant changes (shown with line numbers):\n{}",
                f.display_path, f.content
            );
            Some(coco_messages::wrapping::create_system_reminder_message(
                &text,
            ))
        }
        _ => None,
    }
}

fn build_user_message(user_uuid: Uuid, text: &str, images: &[ImageData]) -> Message {
    if images.is_empty() {
        coco_messages::create_user_message_with_uuid(user_uuid, text)
    } else {
        let mut parts: Vec<UserContentPart> = vec![UserContentPart::text(text)];
        for img in images {
            parts.push(UserContentPart::image(img.bytes.clone(), &img.mime));
        }
        coco_messages::create_user_message_with_parts_and_uuid(user_uuid, parts)
    }
}

#[cfg(test)]
#[path = "at_mention_turn.test.rs"]
mod tests;
