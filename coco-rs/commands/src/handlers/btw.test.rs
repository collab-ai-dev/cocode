use super::*;

#[test]
fn parser_opens_with_or_without_a_question() {
    assert_eq!(BtwRequest::parse(""), BtwRequest::Open);
    assert_eq!(
        BtwRequest::parse("  how does the cache key work?  "),
        BtwRequest::OpenAndAsk {
            question: "how does the cache key work?".to_string(),
        }
    );
}

#[test]
fn handler_is_honest_on_non_tui_surfaces() {
    assert_eq!(
        handler(""),
        "/btw is available only in the interactive TUI."
    );
    assert_eq!(
        handler("what's the diff?"),
        "/btw is available only in the interactive TUI."
    );
}
