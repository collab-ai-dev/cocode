use super::*;

#[test]
fn empty_question_returns_usage() {
    assert_eq!(handler(""), BtwRequest::USAGE);
    assert_eq!(BtwRequest::parse(""), Err(BtwRequest::USAGE));
}

#[test]
fn parser_trims_question_and_recognizes_close() {
    let request = BtwRequest::parse("  how does the cache key work?  ").unwrap();
    assert_eq!(request.question, "how does the cache key work?");
    assert!(!request.is_close());

    assert!(BtwRequest::parse(" --close ").unwrap().is_close());
}

#[test]
fn handler_is_honest_on_non_tui_surfaces() {
    assert_eq!(
        handler("what's the diff?"),
        "/btw is available only in the interactive TUI."
    );
}
