//! Shared `@`-mention resolution + turn-message construction used by every
//! entry path (TUI, headless, AppServer).
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

/// Output of the per-turn user-input resolution pipeline.
/// Field order is the injection order: the user message first
/// (carrying the prompt + any clipboard images), then per-attachment
/// system-reminder messages with file/image/dir content. [`build_messages_for_turn`]
/// concatenates them in that order.
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
    /// Query-owned changed-file reminders. Mention resolution leaves this
    /// empty; production callers pass a `ToolUseContext` and file mentions
    /// are permission-filtered before any content is loaded.
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
/// Steps:
/// 1. `coco_context::process_user_input` — extract `@` mentions.
/// 2. `coco_context::resolve_mentions` — load file content / resolve
/// directory listings, with `FileReadState` dedup.
/// 3. Build the user message (text + optional image parts) with
/// `user_uuid`, then per-attachment reminder messages.
#[cfg(test)]
pub async fn resolve_turn_inputs(
    content: &str,
    images: &[coco_types::QueuedCommandEditImage],
    composer: &coco_types::SubmittedComposer,
    cwd: &Path,
    user_uuid: Uuid,
    file_read_state: &Arc<RwLock<FileReadState>>,
) -> ResolvedTurnInputs {
    resolve_turn_inputs_inner(
        content,
        images,
        composer,
        cwd,
        user_uuid,
        file_read_state,
        None,
    )
    .await
}

pub async fn resolve_turn_inputs_with_permissions(
    content: &str,
    images: &[coco_types::QueuedCommandEditImage],
    composer: &coco_types::SubmittedComposer,
    cwd: &Path,
    user_uuid: Uuid,
    file_read_state: &Arc<RwLock<FileReadState>>,
    tool_context: &coco_tool_runtime::ToolUseContext,
) -> ResolvedTurnInputs {
    resolve_turn_inputs_inner(
        content,
        images,
        composer,
        cwd,
        user_uuid,
        file_read_state,
        Some(tool_context),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resolve_turn_inputs_inner(
    content: &str,
    images: &[coco_types::QueuedCommandEditImage],
    composer: &coco_types::SubmittedComposer,
    cwd: &Path,
    user_uuid: Uuid,
    file_read_state: &Arc<RwLock<FileReadState>>,
    tool_context: Option<&coco_tool_runtime::ToolUseContext>,
) -> ResolvedTurnInputs {
    let mut processed = coco_context::process_user_input(content);

    if let Some(tool_context) = tool_context {
        let mut allowed = Vec::with_capacity(processed.mentions.len());
        for mention in processed.mentions {
            if mention.mention_type != coco_context::user_input::MentionType::FilePath {
                allowed.push(mention);
                continue;
            }
            let path = Path::new(&mention.text);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            let decision =
                coco_tools::tools::read_permissions::check_background_read_permission_with_sandbox(
                    &path,
                    tool_context,
                )
                .await;
            if !matches!(
                decision,
                coco_types::ToolCheckResult::Ask { .. } | coco_types::ToolCheckResult::Deny { .. }
            ) {
                allowed.push(mention);
            }
        }
        processed.mentions = allowed;
    }

    let opts = MentionResolveOptions {
        cwd,
        max_dir_entries: MAX_DIR_ENTRIES,
    };
    let file_attachments =
        coco_context::resolve_mentions(&processed.mentions, file_read_state, &opts).await;

    let mentioned_paths: Vec<std::path::PathBuf> = file_attachments
        .iter()
        .filter_map(|att| match att {
            Attachment::File(f) => Some(std::path::PathBuf::from(&f.filename)),
            Attachment::AlreadyReadFile(f) => Some(std::path::PathBuf::from(&f.filename)),
            _ => None,
        })
        .collect();

    let user_message = build_user_message(user_uuid, content, images, composer);

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

    ResolvedTurnInputs {
        user_message,
        attachment_messages,
        changed_file_messages: Vec::new(),
        mentioned_paths,
    }
}

/// Concatenate the inputs into a `Vec<Message>` in order:
/// `user_message` → file/image/dir reminders → query-owned changed-file notes.
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
/// Produces *two* messages per attachment: a synthetic `tool_use`
/// narration + `tool_result` wrapped in `<system-reminder>`. The image
/// branch keeps the image block unwrapped because `<system-reminder>`
/// only wraps text blocks.
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
            let mut msgs = vec![
                coco_messages::wrapping::create_system_reminder_message(&call),
                coco_messages::wrapping::create_system_reminder_message(&result),
            ];
            // Truncated @-mention content needs a marker so the model knows
            // there's more and reaches for Read rather than assuming it saw
            // the whole file. Kept out of the visible summary (don't narrate).
            if f.truncated {
                let lines = f.content.lines().count();
                let note = format!(
                    "Note: The file {filename} was too large and has been truncated to the first {lines} lines. Don't tell the user about this truncation. Use {read_tool} to read more of the file if you need.",
                    filename = f.filename,
                );
                msgs.push(coco_messages::wrapping::create_system_reminder_message(
                    &note,
                ));
            }
            msgs
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
            let mut image_message =
                coco_messages::create_user_message_with_parts(vec![UserContentPart::File(
                    FilePart::image_base64(b64, &img.media_type),
                )]);
            if let Message::User(user) = &mut image_message {
                user.origin = Some(coco_types::MessageOrigin::SystemInjected);
            }
            vec![
                coco_messages::wrapping::create_system_reminder_message(&call),
                image_message,
            ]
        }
        Attachment::Directory(d) => {
            // directory case: `ls <quoted-abs-path>`
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
        Attachment::PdfReference(p) => {
            // A large @-mentioned PDF can't be inlined — point the model at
            // Read with the pages parameter rather than emitting nothing
            // (which left the model to call Read without pages and fail).
            let content = format!(
                "PDF file: {filename} ({pages} pages, {size}). This PDF is too large to read all at once. You MUST use the {read_tool} tool with the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Do NOT call {read_tool} without the pages parameter or it will fail. Start by reading the first few pages to understand the structure, then read more as needed. Maximum 20 pages per request.",
                filename = p.filename,
                pages = p.page_count,
                size = format_file_size(p.file_size),
            );
            vec![coco_messages::wrapping::create_system_reminder_message(
                &content,
            )]
        }
        Attachment::AlreadyReadFile(_) | Attachment::AgentMention(_) => Vec::new(),
        _ => Vec::new(),
    }
}

/// Human-readable byte size (e.g. `1.2 MB`) for the PDF-reference reminder.
fn format_file_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Build the display-only `@`-mention summary attachment — one compact row
/// per resolved file / directory / image / PDF.
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

fn build_user_message(
    user_uuid: Uuid,
    text: &str,
    images: &[coco_types::QueuedCommandEditImage],
    composer: &coco_types::SubmittedComposer,
) -> Message {
    let composer_is_valid = composer.is_valid_for(text, images.len());
    debug_assert!(
        composer == &coco_types::SubmittedComposer::default() || composer_is_valid,
        "generated submitted composer must match its prompt and images"
    );
    if images.is_empty() && composer == &coco_types::SubmittedComposer::default() {
        coco_messages::create_user_message_with_uuid(user_uuid, text)
    } else {
        let mut text_part = coco_messages::TextContent::new(text);
        if composer != &coco_types::SubmittedComposer::default()
            && composer_is_valid
            && let Ok(value) = serde_json::to_value(composer)
        {
            let mut metadata = coco_llm_types::ProviderMetadata::new();
            metadata.set("coco_submitted_composer", value);
            text_part.provider_metadata = Some(metadata);
        }
        let mut parts: Vec<UserContentPart> = vec![UserContentPart::Text(text_part)];
        for img in images {
            let mime = if img.media_type.is_empty() {
                "image/png"
            } else {
                img.media_type.as_str()
            };
            let mut file = FilePart::image_base64(img.data_base64.clone(), mime);
            let mut metadata = coco_llm_types::ProviderMetadata::new();
            metadata.set(
                "coco_composer_insertion_offset",
                serde_json::json!(img.insertion_offset),
            );
            file.provider_metadata = Some(metadata);
            parts.push(UserContentPart::File(file));
        }
        coco_messages::create_user_message_with_parts_and_uuid(user_uuid, parts)
    }
}

#[cfg(test)]
#[path = "at_mention_turn.test.rs"]
mod tests;
