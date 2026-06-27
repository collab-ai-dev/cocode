//! View-string snapshot tests for the `/add-dir` interactive overlay.
//!
//! Input mutation lives in `modal_pane/add_directory.rs`; this file pins the
//! emitted body text for the empty, typed, and error states so cosmetic
//! regressions surface as `insta` diffs.

use super::*;
use crate::state::WizardTextField;
use crate::theme::Theme;

fn body_only(s: &AddDirectoryState) -> String {
    let _locale = crate::i18n::locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let (_title, body, _color) = add_directory_content(s, styles);
    body
}

#[test]
fn snapshot_add_dir_empty() {
    let s = AddDirectoryState::new();
    insta::assert_snapshot!("add_dir_empty", body_only(&s));
}

#[test]
fn snapshot_add_dir_with_input() {
    let mut s = AddDirectoryState::new();
    s.input = WizardTextField::seeded("/tmp/project");
    insta::assert_snapshot!("add_dir_input", body_only(&s));
}

#[test]
fn snapshot_add_dir_error() {
    let mut s = AddDirectoryState::new();
    s.input = WizardTextField::seeded("/nope");
    s.error = Some("Cannot add directory `/nope`: not a directory".to_string());
    insta::assert_snapshot!("add_dir_error", body_only(&s));
}
