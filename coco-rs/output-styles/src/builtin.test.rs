use super::*;
use pretty_assertions::assert_eq;

#[test]
fn builtin_catalog_has_two_entries_in_ts_order() {
    let styles = builtin_styles();
    assert_eq!(styles.len(), 2);
    assert_eq!(styles[0].name, EXPLANATORY_STYLE_NAME);
    assert_eq!(styles[1].name, LEARNING_STYLE_NAME);
}

#[test]
fn builtins_keep_coding_instructions_true() {
    for style in builtin_styles() {
        assert_eq!(
            style.keep_coding_instructions,
            Some(true),
            "{} must keep the doing-tasks section on top",
            style.name
        );
    }
}

#[test]
fn explanatory_includes_insight_block() {
    let explanatory = builtin_styles()
        .into_iter()
        .find(|s| s.name == EXPLANATORY_STYLE_NAME)
        .unwrap();
    assert!(explanatory.prompt.contains("# Explanatory Style Active"));
    assert!(explanatory.prompt.contains("## Insights"));
    assert!(explanatory.prompt.contains("Insight ──"));
}

#[test]
fn learning_includes_request_format_and_examples() {
    let learning = builtin_styles()
        .into_iter()
        .find(|s| s.name == LEARNING_STYLE_NAME)
        .unwrap();
    assert!(learning.prompt.contains("# Learning Style Active"));
    assert!(learning.prompt.contains("**Learn by Doing**"));
    assert!(learning.prompt.contains("TODO(human)"));
    // Verbatim phrase — protects against accidental rewrites.
    assert!(learning.prompt.contains("Whole Function Example"));
    assert!(learning.prompt.contains("Partial Function Example"));
    assert!(learning.prompt.contains("Debugging Example"));
}
