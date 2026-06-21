//! Tests for the clipboard-related update handlers
//! (copy-last-message + paste-from-clipboard).

use pretty_assertions::assert_eq;

use super::copy_last_message_with;
use super::paste_from_clipboard_with;
use crate::state::AppState;
use crate::state::ui::ToastSeverity;
use coco_tui_ui::clipboard_copy::ClipboardLease;
use coco_tui_ui::paste::ImageData;

#[test]
fn surfaces_info_toast_when_no_agent_response_cached() {
    let mut state = AppState::new();
    copy_last_message_with(&mut state, |_| panic!("copy_fn must not be called"));
    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Info);
    assert!(
        state.ui.toasts[0].message.contains("No agent response"),
        "unexpected toast message: {}",
        state.ui.toasts[0].message
    );
    assert!(state.ui.clipboard_lease.is_none());
}

#[test]
fn success_toast_with_lease_reports_until_exit() {
    let mut state = AppState::new();
    state.session.last_agent_markdown = Some("hello world".to_string());

    copy_last_message_with(&mut state, |text| {
        assert_eq!(text, "hello world");
        Ok(Some(ClipboardLease::test()))
    });

    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Success);
    // Toast communicates both the character count and the Linux-arboard
    // lease's lifetime caveat ("until exit") so users can decide whether to
    // paste before quitting.
    assert_eq!(state.ui.toasts[0].message, "Copied 11 chars (until exit)");
    assert!(state.ui.clipboard_lease.is_some());
}

#[test]
fn success_toast_without_lease_reports_persistent() {
    let mut state = AppState::new();
    state.session.last_agent_markdown = Some("one two three".to_string());

    copy_last_message_with(&mut state, |_| Ok(None));

    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Success);
    // No lease means OSC 52 or a non-Linux native clipboard — the copied
    // text survives TUI exit.
    assert_eq!(state.ui.toasts[0].message, "Copied 13 chars (persistent)");
    assert!(state.ui.clipboard_lease.is_none());
}

#[test]
fn error_toast_when_copy_backend_fails() {
    let mut state = AppState::new();
    state.session.last_agent_markdown = Some("payload".to_string());

    copy_last_message_with(&mut state, |_| Err("clipboard unavailable".to_string()));

    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Error);
    assert!(
        state.ui.toasts[0].message.contains("clipboard unavailable"),
        "unexpected toast message: {}",
        state.ui.toasts[0].message
    );
    assert!(state.ui.clipboard_lease.is_none());
}

#[test]
fn empty_agent_markdown_is_treated_as_missing() {
    let mut state = AppState::new();
    state.session.last_agent_markdown = Some(String::new());

    copy_last_message_with(&mut state, |_| panic!("copy_fn must not be called"));

    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Info);
}

// ── PasteFromClipboard / image paste ──

#[tokio::test]
async fn paste_inserts_pill_at_cursor_and_registers_image() {
    let mut state = AppState::new();
    // Pre-position the cursor in the middle of existing text so we verify
    // the pill is inserted at the cursor, not appended.
    state.ui.input.textarea.set_text("prefix suffix");
    state.ui.input.textarea.set_cursor(7); // between "prefix " and "suffix"

    let image = ImageData {
        bytes: vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], // PNG magic
        mime: "image/png".to_string(),
    };
    paste_from_clipboard_with(&mut state, || async { Ok(Some(image)) }).await;

    // The pill label sits exactly where the cursor was.
    assert_eq!(state.ui.input.text(), "prefix [Image #1]suffix");
    // Cursor advanced past the inserted pill (byte offset == char count
    // for ASCII pill label).
    assert_eq!(state.ui.input.textarea.cursor(), "prefix [Image #1]".len());
    // The bytes were registered so resolve_structured can later detach them.
    assert_eq!(state.ui.paste_manager.entries().len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Success);
    assert!(state.ui.toasts[0].message.starts_with("Attached image"));
}

#[tokio::test]
async fn paste_pill_round_trips_through_resolve_structured() {
    // End-to-end check: paste an image, then exercise the submit path's
    // structured resolver to confirm the pill is extracted back into a
    // `UserCommand::SubmitInput.images[0]` payload.
    let mut state = AppState::new();
    let png = vec![0x89, 0x50, 0x4e, 0x47];
    paste_from_clipboard_with(&mut state, || {
        let png = png.clone();
        async move {
            Ok(Some(ImageData {
                bytes: png,
                mime: "image/png".to_string(),
            }))
        }
    })
    .await;

    // Simulate what `edit::submit` does with the composed prompt.
    let prompt = format!("{} please analyze", state.ui.input.text());
    let resolved = state.ui.paste_manager.resolve_structured(&prompt);

    // The `[Image #N]` placeholder is kept inline (so the transcript can echo
    // it); the image bytes are extracted into the separate content block.
    assert_eq!(resolved.text, "[Image #1] please analyze");
    assert_eq!(resolved.images.len(), 1);
    assert_eq!(resolved.images[0].mime, "image/png");
    assert_eq!(resolved.images[0].bytes, png);
}

#[tokio::test]
async fn paste_shows_info_toast_when_clipboard_has_no_image() {
    let mut state = AppState::new();
    paste_from_clipboard_with(&mut state, || async { Ok(None) }).await;

    assert!(state.ui.input.is_empty());
    assert!(state.ui.paste_manager.entries().is_empty());
    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Info);
    // Tells the user *why* nothing happened + points at the alternative.
    assert!(
        state.ui.toasts[0].message.contains("No image"),
        "unexpected toast message: {}",
        state.ui.toasts[0].message
    );
}

#[tokio::test]
async fn paste_shows_error_toast_when_backend_fails() {
    let mut state = AppState::new();
    paste_from_clipboard_with(&mut state, || async {
        Err(std::io::Error::other("xclip not installed"))
    })
    .await;

    assert!(state.ui.input.is_empty());
    assert!(state.ui.paste_manager.entries().is_empty());
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Error);
    assert!(
        state.ui.toasts[0].message.contains("xclip not installed"),
        "unexpected toast message: {}",
        state.ui.toasts[0].message
    );
}

#[test]
fn test_normalize_pasted_path_handles_quotes_escapes_and_file_urls() {
    use super::normalize_pasted_path;

    assert_eq!(
        normalize_pasted_path("/home/u/img.png"),
        Some(std::path::PathBuf::from("/home/u/img.png"))
    );
    assert_eq!(
        normalize_pasted_path("'/home/u/my img.png'"),
        Some(std::path::PathBuf::from("/home/u/my img.png"))
    );
    assert_eq!(
        normalize_pasted_path("\"/home/u/my img.png\""),
        Some(std::path::PathBuf::from("/home/u/my img.png"))
    );
    assert_eq!(
        normalize_pasted_path("/home/u/my\\ img.png"),
        Some(std::path::PathBuf::from("/home/u/my img.png"))
    );
    assert_eq!(
        normalize_pasted_path("file:///home/u/my%20img.png"),
        Some(std::path::PathBuf::from("/home/u/my img.png"))
    );
    assert_eq!(
        normalize_pasted_path("file://localhost/home/u/img.png"),
        Some(std::path::PathBuf::from("/home/u/img.png"))
    );
    // Multi-line pastes are never a single path token.
    assert_eq!(normalize_pasted_path("/a.png\n/b.png"), None);
    assert_eq!(normalize_pasted_path("   "), None);
}

#[test]
fn test_image_mime_for_path_uses_extension_allowlist() {
    use super::image_mime_for_path;

    let mime = |p: &str| image_mime_for_path(std::path::Path::new(p));
    assert_eq!(mime("/a/b.png"), Some("image/png"));
    assert_eq!(mime("/a/b.JPG"), Some("image/jpeg"));
    assert_eq!(mime("/a/b.jpeg"), Some("image/jpeg"));
    assert_eq!(mime("/a/b.gif"), Some("image/gif"));
    assert_eq!(mime("/a/b.webp"), Some("image/webp"));
    assert_eq!(mime("/a/b.bmp"), Some("image/bmp"));
    assert_eq!(mime("/a/b.txt"), None);
    assert_eq!(mime("/a/b"), None);
}

#[test]
fn test_sniff_image_path_paste_reads_real_file_and_rejects_others() {
    use super::sniff_image_path_paste;

    let dir = std::env::temp_dir().join(format!("coco-paste-sniff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let img = dir.join("shot.png");
    std::fs::write(&img, [0x89, 0x50, 0x4e, 0x47]).expect("write temp image");

    let sniffed = sniff_image_path_paste(img.to_str().expect("utf8 path"));
    assert_eq!(
        sniffed,
        Some((vec![0x89, 0x50, 0x4e, 0x47], "image/png".to_string()))
    );

    // Nonexistent file and plain prose both fall through to text paste.
    assert_eq!(
        sniff_image_path_paste(dir.join("missing.png").to_str().expect("utf8")),
        None
    );
    assert_eq!(sniff_image_path_paste("just some pasted prose"), None);

    let _ = std::fs::remove_dir_all(&dir);
}
