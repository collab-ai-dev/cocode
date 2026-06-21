//! Clipboard-related command handlers.
//!
//! [`copy_last_message`] backs `/copy`.
//! [`paste_from_clipboard`] backs `Ctrl+V` — the image-paste path for
//! screenshot → multimodal-agent flows. Plain-text paste is owned by the
//! terminal's bracketed-paste (Cmd+V / Ctrl+Shift+V), so this handler is
//! exclusively about pulling an image off the system clipboard.

use crate::i18n::t;
use crate::state::AppState;
use crate::state::ui::Toast;
use coco_tui_ui::clipboard_copy;
use coco_tui_ui::paste::ImageData;

/// Copy the last agent response (raw markdown) to the system clipboard and
/// surface the result as a toast. Mirrors codex-rs
/// `ChatWidget::copy_last_agent_markdown`.
pub(super) fn copy_last_message(state: &mut AppState) {
    copy_last_message_with(state, clipboard_copy::copy_to_clipboard);
}

/// Inner implementation with an injectable backend — exposed to the update
/// tests so they can stub out real clipboard/tty I/O.
pub(super) fn copy_last_message_with(
    state: &mut AppState,
    copy_fn: impl FnOnce(&str) -> Result<Option<clipboard_copy::ClipboardLease>, String>,
) {
    let Some(markdown) = state.session.last_agent_markdown.clone() else {
        state
            .ui
            .add_toast(Toast::info(t!("toast.no_agent_response").to_string()));
        return;
    };
    if markdown.is_empty() {
        state
            .ui
            .add_toast(Toast::info(t!("toast.no_agent_response").to_string()));
        return;
    }
    let char_count = markdown.chars().count();
    match copy_fn(&markdown) {
        Ok(lease) => {
            // On Linux the arboard path returns a lease whose drop-point
            // defines when the clipboard content disappears; without a lease
            // we either used OSC 52 (terminal-owned, survives exit) or a
            // non-Linux native clipboard (OS-owned, survives exit). Tell the
            // user which one they got so they can plan for app exit.
            let durability = if lease.is_some() {
                t!("toast.copy_durability_until_exit")
            } else {
                t!("toast.copy_durability_persistent")
            };
            state.ui.clipboard_lease = lease;
            state.ui.add_toast(Toast::success(
                t!(
                    "toast.copied_chars",
                    count = char_count,
                    durability = durability
                )
                .to_string(),
            ));
        }
        Err(err) => {
            state.ui.add_toast(Toast::error(
                t!("toast.copy_failed_short", error = err).to_string(),
            ));
        }
    }
}

/// Read an image from the system clipboard and insert a `[Image #N]` pill
/// at the cursor. On submit, `PasteManager::resolve_structured` pulls the
/// bytes out of the pill and attaches them to `UserCommand::SubmitInput.images`
/// so a multimodal agent sees the screenshot alongside the user's prompt.
///
/// Text paste intentionally stays with the terminal's bracketed-paste flow
/// (handled by `TuiEvent::Paste`) — this path is image-only.
pub(super) async fn paste_from_clipboard(state: &mut AppState) {
    paste_from_clipboard_with(state, coco_tui_ui::clipboard::read_clipboard_image).await;
}

/// Inner implementation with an injectable reader so tests can avoid the
/// real `xclip` / `osascript` subprocesses.
pub(super) async fn paste_from_clipboard_with<F, Fut>(state: &mut AppState, read_fn: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Option<ImageData>>>,
{
    match read_fn().await {
        Ok(Some(image)) => {
            let size_kb = image.bytes.len().div_ceil(1024);
            let pill = state
                .ui
                .paste_manager
                .add_image_data(image.bytes, image.mime);
            state.ui.input.textarea.insert_str(&pill);
            state.ui.add_toast(Toast::success(
                t!("toast.image_attached", size_kb = size_kb).to_string(),
            ));
        }
        Ok(None) => {
            state
                .ui
                .add_toast(Toast::info(t!("toast.no_image_clipboard").to_string()));
        }
        Err(e) => {
            state.ui.add_toast(Toast::error(
                t!("toast.paste_failed", error = e.to_string()).to_string(),
            ));
        }
    }
}

/// Detect a bracketed paste that is a path to an image file (drag-and-drop
/// onto the terminal) and load its bytes for an image-pill attach. Returns
/// `None` — falling through to the text-paste path — unless the paste is a
/// single path token with an image extension whose file reads successfully.
/// Mirrors codex-rs `handle_paste_image_path` / `is_image_path`.
pub(crate) fn sniff_image_path_paste(text: &str) -> Option<(Vec<u8>, String)> {
    let path = normalize_pasted_path(text)?;
    let mime = image_mime_for_path(&path)?;
    let bytes = std::fs::read(&path).ok()?;
    Some((bytes, mime.to_string()))
}

/// Normalize a pasted path token: surrounding quotes (Finder / file
/// managers), `file://` URLs with percent-encoding (GTK drag-drop), shell
/// backslash-escaped spaces (terminal drag-drop), and `~/` expansion.
fn normalize_pasted_path(text: &str) -> Option<std::path::PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    let unquoted = trimmed
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(trimmed);
    if let Some(rest) = unquoted.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        return Some(std::path::PathBuf::from(percent_decode(path)));
    }
    let unescaped = unquoted.replace("\\ ", " ");
    if let Some(rest) = unescaped.strip_prefix("~/") {
        let home = std::env::var_os("HOME")?;
        return Some(std::path::PathBuf::from(home).join(rest));
    }
    Some(std::path::PathBuf::from(unescaped))
}

/// Minimal percent-decoder for `file://` URL paths (UTF-8 lossy on the
/// decoded bytes — pasted paths are display strings, not security inputs).
fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Image MIME by file extension — the same allowlist codex's
/// `is_image_path` uses.
fn image_mime_for_path(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "clipboard.test.rs"]
mod tests;
