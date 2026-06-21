use super::*;
use pretty_assertions::assert_eq;

use crate::i18n::locale_test_guard;
use crate::state::DiffViewState;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;

#[test]
fn diff_view_content_formats_diff_lines_and_clamps_negative_scroll() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = DiffViewState {
        path: "src/lib.rs".to_string(),
        diff: "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
 context"
            .to_string(),
        scroll: -4,
    };

    let (title, body, border) = diff_view_content(&state, UiStyles::new(&theme));

    assert_eq!(title, " Diff: src/lib.rs [1/6] ");
    assert_eq!(border, theme.primary);
    assert!(body.contains("    --- a/src/lib.rs"));
    assert!(body.contains("    +++ b/src/lib.rs"));
    assert!(body.contains("  @@ -1,2 +1,2 @@"));
    assert!(body.contains("  - old"));
    assert!(body.contains("  + new"));
}

#[test]
fn diff_view_content_scrolls_and_caps_to_thirty_lines() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = DiffViewState {
        path: "src/lib.rs".to_string(),
        diff: (0..35)
            .map(|i| format!(" line-{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
        scroll: 3,
    };

    let (title, body, _) = diff_view_content(&state, UiStyles::new(&theme));

    assert_eq!(title, " Diff: src/lib.rs [4/35] ");
    assert!(body.lines().any(|line| line == "    line-3"));
    assert!(body.lines().any(|line| line == "    line-32"));
    assert!(!body.lines().any(|line| line == "    line-2"));
    assert!(!body.lines().any(|line| line == "    line-33"));
}
