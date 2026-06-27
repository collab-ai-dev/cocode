//! Unit tests for [`StashNotice`] visibility + preview truncation.

use super::StashNotice;
use crate::state::ui::StashedInput;
use coco_tui_ui::style::UiStyles;
use pretty_assertions::assert_eq;

fn stash_with(text: &str) -> StashedInput {
    StashedInput {
        text: text.to_string(),
        cursor_byte: text.len(),
        paste_entries: Vec::new(),
    }
}

#[test]
fn should_display_when_stash_holds_content() {
    let stash = stash_with("hello");
    assert!(StashNotice::should_display(Some(&stash)));
}

#[test]
fn should_not_display_when_none() {
    assert!(!StashNotice::should_display(None));
}

#[test]
fn should_not_display_when_only_whitespace() {
    let stash = stash_with("   \n\t  ");
    assert!(!StashNotice::should_display(Some(&stash)));
}

#[test]
fn truncated_preview_collapses_to_first_line() {
    let theme = crate::theme::Theme::default();
    let stash = stash_with("line one\nline two\nline three");
    let widget = StashNotice::new(&stash, UiStyles::new(&theme));
    assert_eq!(widget.truncated_preview(), "line one");
}

#[test]
fn truncated_preview_caps_at_40_chars_with_ellipsis() {
    let theme = crate::theme::Theme::default();
    let long = "a".repeat(80);
    let stash = stash_with(&long);
    let widget = StashNotice::new(&stash, UiStyles::new(&theme));
    let preview = widget.truncated_preview();
    let chars: Vec<char> = preview.chars().collect();
    // 40 'a's + 1 ellipsis = 41 visible chars.
    assert_eq!(chars.len(), 41);
    assert_eq!(chars[40], '…');
}
