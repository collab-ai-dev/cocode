//! Helpers for the `/copy` command: assistant-message lookback and
//! fenced-code-block extraction.

#[cfg(test)]
#[path = "copy.test.rs"]
mod tests;

use std::collections::HashSet;
use std::path::PathBuf;

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::SystemMessageLevel;
use uuid::Uuid;

use crate::command::SystemPushKind;
use crate::command::UserCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::CopyPickerSelection;
use crate::state::CopyPickerState;
use crate::state::transcript_view::TranscriptView;
use crate::state::ui::Toast;
use coco_tui_ui::clipboard_copy;

const RESPONSE_FILENAME: &str = "response.md";

pub const MAX_LOOKBACK: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    pub code: String,
    pub lang: Option<String>,
}

/// Walk the transcript newest-first, gathering raw markdown from each
/// distinct assistant message that produced visible text. Dedups by
/// `message_uuid` (one Message → many cells), skips api-error turns
/// and tool-only turns. Caps at `max`. Index 0 in the returned slice
/// is the latest.
pub fn collect_recent_assistant_texts(transcript: &TranscriptView, max: usize) -> Vec<String> {
    if max == 0 {
        return Vec::new();
    }
    let mut texts = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    for cell in transcript.cells().iter().rev() {
        if texts.len() >= max {
            break;
        }
        if !seen.insert(cell.message_uuid) {
            continue;
        }
        let Message::Assistant(asst) = cell.source.as_ref() else {
            continue;
        };
        if asst.api_error.is_some() {
            continue;
        }
        let LlmMessage::Assistant { content, .. } = &asst.message else {
            continue;
        };
        let parts: Vec<&str> = content
            .iter()
            .filter_map(|part| match part {
                AssistantContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }
        texts.push(parts.join("\n\n"));
    }
    texts
}

/// Pull every fenced ```code``` block out of a markdown source after
/// stripping prompt XML tags. Covers prompt-tag stripping and
/// CommonMark-style fenced blocks without adding a Markdown-parser
/// dependency to the TUI crate. An opened-but-unterminated block at
/// EOF is closed implicitly so the trailing block still appears in the
/// picker.
pub fn extract_code_blocks(markdown: &str) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let stripped = strip_prompt_xml_tags(markdown);
    let mut current: Option<OpenFence<'_>> = None;
    for raw_line in stripped.lines() {
        if let Some(open) = current.as_mut() {
            if is_closing_fence(raw_line, open.fence_char, open.fence_len) {
                if let Some(open) = current.take() {
                    blocks.push(CodeBlock {
                        code: open.lines.join("\n"),
                        lang: open.lang,
                    });
                }
            } else {
                open.lines.push(raw_line);
            }
        } else if let Some((fence_char, fence_len, label)) = opening_fence(raw_line) {
            current = Some(OpenFence {
                fence_char,
                fence_len,
                lang: (!label.is_empty()).then(|| label.to_string()),
                lines: Vec::new(),
            });
        }
    }
    if let Some(open) = current {
        blocks.push(CodeBlock {
            code: open.lines.join("\n"),
            lang: open.lang,
        });
    }
    blocks
}

struct OpenFence<'a> {
    fence_char: char,
    fence_len: usize,
    lang: Option<String>,
    lines: Vec<&'a str>,
}

fn opening_fence(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let fence_char = match trimmed.chars().next()? {
        '`' => '`',
        '~' => '~',
        _ => return None,
    };
    let fence_len = trimmed.chars().take_while(|c| *c == fence_char).count();
    if fence_len < 3 {
        return None;
    }
    Some((fence_char, fence_len, trimmed[fence_len..].trim()))
}

fn is_closing_fence(line: &str, fence_char: char, fence_len: usize) -> bool {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return false;
    }
    let trimmed = &line[indent..];
    let len = trimmed.chars().take_while(|c| *c == fence_char).count();
    len >= fence_len && trimmed[len..].trim().is_empty()
}

fn strip_prompt_xml_tags(content: &str) -> String {
    let mut stripped = content.to_string();
    for tag in [
        "commit_analysis",
        "context",
        "function_analysis",
        "pr_analysis",
    ] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = stripped.find(&open) {
            let search_from = start + open.len();
            let Some(relative_end) = stripped[search_from..].find(&close) else {
                break;
            };
            let mut end = search_from + relative_end + close.len();
            if stripped[end..].starts_with('\n') {
                end += 1;
            }
            stripped.replace_range(start..end, "");
        }
    }
    stripped.trim().to_string()
}

/// File-extension suffix for a code-block fence language tag.
/// Alnum-only sanitization (defends against path-traversal in
/// pathological fence labels like ```` ```../etc/passwd ````),
/// empty / `plaintext` collapse to `.txt`. Returns `".txt"` when
/// `lang` is `None`.
pub fn file_extension(lang: Option<&str>) -> String {
    let Some(lang) = lang else {
        return ".txt".to_string();
    };
    let sanitized: String = lang.chars().filter(char::is_ascii_alphanumeric).collect();
    if sanitized.is_empty() || sanitized == "plaintext" {
        ".txt".to_string()
    } else {
        format!(".{sanitized}")
    }
}

/// Drive the `/copy [N]` slash command: parse the arg, walk the
/// transcript newest-first, decide between a direct clipboard write
/// and mounting the [`ModalState::CopyPicker`] surface based on
/// `copy_full_response` + presence of code blocks.
pub(crate) fn handle_copy_command(state: &mut AppState, args: &str) -> Option<String> {
    let age = match parse_copy_arg(args) {
        Ok(age) => age,
        Err(err) => {
            state.ui.add_toast(Toast::error(err.clone()));
            return Some(err);
        }
    };

    let texts = collect_recent_assistant_texts(&state.session.transcript, MAX_LOOKBACK);
    if texts.is_empty() {
        let message = t!("toast.no_agent_response").to_string();
        state.ui.add_toast(Toast::info(message.clone()));
        return Some(message);
    }
    if age >= texts.len() {
        let count = texts.len();
        let message = t!("toast.copy_only_n_available", count = count).to_string();
        state.ui.add_toast(Toast::warning(message.clone()));
        return Some(message);
    }

    let text = texts[age].clone();
    let blocks = extract_code_blocks(&text);
    if blocks.is_empty() || state.ui.display_settings.copy_full_response {
        return Some(copy_text_to_clipboard(state, &text, RESPONSE_FILENAME));
    }

    state
        .ui
        .show_modal(crate::state::ModalState::CopyPicker(CopyPickerState {
            full_text: text,
            code_blocks: blocks
                .into_iter()
                .map(|b| crate::state::CopyPickerCodeBlock {
                    code: b.code,
                    lang: b.lang,
                })
                .collect(),
            message_age: age,
            selected: CopyPickerSelection::Full,
        }));
    None
}

/// Parse the `[N]` arg passed to `/copy`. Empty → age=0 (latest).
/// Non-empty must be an integer >= 1; returns 0-indexed age = N-1.
fn parse_copy_arg(args: &str) -> Result<usize, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n - 1),
        _ => Err(t!("toast.copy_usage_invalid", arg = trimmed).to_string()),
    }
}

/// Copy arbitrary text to the system clipboard, manage the platform
/// lease, **also** dump it to `<tmpdir>/coco/<filename>` as a fallback
/// (OSC 52 isn't universally supported, and the user can `cat` the
/// file when the terminal silently drops the escape), and surface a
/// toast. Used by `/copy` (direct copy path) and the picker confirm.
pub(crate) fn copy_text_to_clipboard(state: &mut AppState, text: &str, filename: &str) -> String {
    copy_text_with(
        state,
        text,
        filename,
        clipboard_copy::copy_to_clipboard,
        write_to_temp_file,
    )
}

pub(crate) fn copy_text_with(
    state: &mut AppState,
    text: &str,
    filename: &str,
    copy_fn: impl FnOnce(&str) -> Result<Option<clipboard_copy::ClipboardLease>, String>,
    write_fn: impl FnOnce(&str, &str) -> Result<PathBuf, std::io::Error>,
) -> String {
    if text.is_empty() {
        let message = t!("toast.no_agent_response").to_string();
        state.ui.add_toast(Toast::info(message.clone()));
        return message;
    }
    let char_count = text.chars().count();
    let line_count = text.matches('\n').count() + 1;
    let file_path = write_fn(text, filename).ok();

    match copy_fn(text) {
        Ok(lease) => {
            let durability = if lease.is_some() {
                t!("toast.copy_durability_until_exit")
            } else {
                t!("toast.copy_durability_persistent")
            };
            state.ui.clipboard_lease = lease;
            let body = match file_path {
                Some(path) => t!(
                    "toast.copied_chars_with_file",
                    count = char_count,
                    lines = line_count,
                    durability = durability,
                    path = path.display().to_string().as_str(),
                )
                .to_string(),
                None => t!(
                    "toast.copied_chars",
                    count = char_count,
                    durability = durability
                )
                .to_string(),
            };
            state.ui.add_toast(Toast::success(body.clone()));
            body
        }
        Err(err) => {
            let message = t!("toast.copy_failed_short", error = err).to_string();
            state.ui.add_toast(Toast::error(message.clone()));
            message
        }
    }
}

/// Write `text` to `<tmpdir>/coco/<filename>`, creating the directory
/// as needed.
fn write_to_temp_file(text: &str, filename: &str) -> Result<PathBuf, std::io::Error> {
    let dir = std::env::temp_dir().join("coco");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(filename);
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Apply the picker's confirmed selection: copy the right slice to the
/// clipboard, and — for the `Always` arm — persist
/// `copy_full_response: true` to user settings so the next `/copy`
/// skips the picker entirely.
pub(crate) fn confirm_picker_selection(
    state: &mut AppState,
    cp: CopyPickerState,
) -> Option<String> {
    let (text, filename, persist_always) = picker_selection_content(&cp)?;
    let mut result = copy_text_to_clipboard(state, text, &filename);

    if !persist_always {
        return Some(result);
    }
    match coco_config::global_config::write_user_setting(
        coco_config::settings::COPY_FULL_RESPONSE_KEY,
        serde_json::json!(true),
    ) {
        Ok(_) => {
            let next = state
                .ui
                .display_settings
                .clone()
                .with_copy_full_response(true);
            state.ui.apply_display_settings(next);
            let message = t!("toast.copy_preference_saved").to_string();
            state.ui.add_toast(Toast::info(message.clone()));
            result.push('\n');
            result.push_str(&message);
        }
        Err(err) => {
            let message =
                t!("toast.copy_preference_save_failed", error = err.to_string()).to_string();
            state.ui.add_toast(Toast::warning(message.clone()));
            result.push('\n');
            result.push_str(&message);
        }
    }
    Some(result)
}

pub(crate) fn write_picker_selection_to_file(
    state: &mut AppState,
    cp: CopyPickerState,
) -> Option<String> {
    let (text, filename, _) = picker_selection_content(&cp)?;
    match write_to_temp_file(text, &filename) {
        Ok(path) => {
            let path = path.display().to_string();
            let message = t!("toast.copy_written_to_file", path = path.as_str()).to_string();
            state.ui.add_toast(Toast::success(message.clone()));
            Some(message)
        }
        Err(err) => {
            let message = t!(
                "toast.copy_write_file_failed",
                error = err.to_string().as_str()
            )
            .to_string();
            state.ui.add_toast(Toast::error(message.clone()));
            Some(message)
        }
    }
}

fn picker_selection_content(cp: &CopyPickerState) -> Option<(&str, String, bool)> {
    match cp.selected {
        CopyPickerSelection::Full => Some((&cp.full_text, RESPONSE_FILENAME.to_string(), false)),
        CopyPickerSelection::CodeBlock(idx) => {
            let block = cp.code_blocks.get(idx)?;
            let ext = file_extension(block.lang.as_deref());
            Some((&block.code, format!("copy{ext}"), false))
        }
        CopyPickerSelection::Always => Some((&cp.full_text, RESPONSE_FILENAME.to_string(), true)),
    }
}

pub(crate) fn enqueue_copy_output(
    message: String,
    command_tx: &tokio::sync::mpsc::Sender<UserCommand>,
) {
    if let Err(e) = command_tx.try_send(UserCommand::PushSystemMessage {
        kind: SystemPushKind::Informational {
            level: SystemMessageLevel::Info,
            title: String::new(),
            message,
        },
    }) {
        tracing::warn!(
            target: "coco_tui::copy",
            error = ?e,
            "enqueue_copy_output: failed to dispatch PushSystemMessage",
        );
    }
}
