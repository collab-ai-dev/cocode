use super::compact_boundary_text;

/// The compact-boundary line must interpolate the resolved shortcut
/// verbatim — independent of which locale (`en`, `zh-CN`, …) happens
/// to be active when the test runs. We only assert the
/// locale-independent contract: the shortcut argument appears in the
/// rendered string. The localized prefix ("Conversation compacted" /
/// "对话已压缩") is exercised by the i18n snapshot tests.
#[test]
fn compact_boundary_text_interpolates_the_shortcut() {
    let text = compact_boundary_text("ctrl+o");
    assert!(text.contains("ctrl+o"));
    assert!(!text.is_empty());
}
