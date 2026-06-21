//! Render the `/add-dir` (no-argument) interactive directory-input overlay.
//!
//! View-string composition only; input mutation lives in
//! `modal_pane/add_directory.rs` and the state shape in
//! `state/surface_payloads.rs`. Mirrors the `/permissions` add-form input row
//! (caret glyph + prompt + help) for visual consistency.

use ratatui::style::Color;

use crate::i18n::t;
use crate::state::AddDirectoryState;
use crate::state::WizardTextField;
use coco_tui_ui::style::UiStyles;

/// Caret glyph between the before / after halves of the input — matches the
/// permissions editor and agents wizard.
const CARET_GLYPH: char = '▏';

/// Render the add-directory overlay. Returns `(title, body, border)`. The
/// border turns red while a validation error is showing so the failed state
/// reads at a glance even though the text body is monochrome.
pub(crate) fn add_directory_content(
    s: &AddDirectoryState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let title = t!("dialog.title_add_dir").to_string();

    let mut body = String::new();
    body.push_str(&t!("dialog.add_dir_prompt"));
    body.push('\n');
    body.push_str(&format!("  > {}\n", caret_render(&s.input)));
    body.push('\n');
    body.push_str(&t!("dialog.add_dir_help"));

    if let Some(err) = &s.error {
        body.push_str("\n\n  ");
        body.push_str(err);
    }

    let border = if s.error.is_some() {
        styles.error()
    } else {
        styles.primary()
    };
    (title, body, border)
}

fn caret_render(field: &WizardTextField) -> String {
    let (before, after) = field.split_at_cursor();
    format!("{before}{CARET_GLYPH}{after}")
}

#[cfg(test)]
#[path = "add_directory.test.rs"]
mod tests;
