use super::*;

#[test]
fn prompt_projects_live_user_contexts_and_default_bindings() {
    let prompt = prompt();

    for context in coco_keybindings::KeybindingContext::ALL_USER {
        assert!(
            prompt.contains(&format!("| `{context}` | {} |", context.description())),
            "missing context {context}"
        );
    }
    assert!(prompt.contains("| `select:accept` | `enter` | Settings |"));
    assert!(prompt.contains("| `select:next` | `ctrl+n`, `down` | Settings |"));
    assert!(prompt.contains("| `theme:toggleSyntaxHighlighting` | `ctrl+t` | Settings |"));
    assert!(prompt.contains("| `select:next` | `ctrl+n`, `down`, `j` | Select |"));
}

#[test]
fn prompt_excludes_internal_contexts_and_stale_static_advice() {
    let prompt = prompt();

    assert!(!prompt.contains("| `Scroll` |"));
    assert!(!prompt.contains("| `MessageActions` |"));
    assert!(!prompt.contains("| `confirm:no` | `escape` | Settings |"));
    assert!(!prompt.contains("| `select:accept` | `enter`, `space` | Select |"));
    assert!(!prompt.contains("__AVAILABLE_CONTEXTS__"));
    assert!(!prompt.contains("__DEFAULT_BINDINGS__"));
}
