use super::*;

#[test]
fn test_handler_empty_question_returns_usage() {
    let out = handler("");
    assert!(out.starts_with("Usage:"), "empty arg gives usage: {out}");
    // Must NOT emit the sentinel — runner should display the usage
    // text verbatim, not interpret it as a fork request.
    assert!(!out.contains(BTW_SENTINEL));
}

#[test]
fn test_handler_emits_sentinel_with_question() {
    let out = handler("how does the cache key work?");
    assert!(out.starts_with(BTW_SENTINEL));
    assert!(out.contains("how does the cache key work?"));
}

#[test]
fn test_parse_sentinel_extracts_question() {
    let req = parse_btw_sentinel(&handler("what's the diff?")).expect("must parse");
    assert_eq!(req.question, "what's the diff?");
}

#[test]
fn test_parse_sentinel_returns_none_for_non_sentinel_output() {
    assert!(parse_btw_sentinel("Usage: /btw <question>").is_none());
    assert!(parse_btw_sentinel("Hello world").is_none());
}

#[test]
fn test_parse_sentinel_returns_none_for_empty_question() {
    // Defence-in-depth: if a runner ever stuffs `__COCO_BTW_NOW__ ` into
    // the output without a question, the parser must reject it rather than
    // launch an empty fork.
    assert!(parse_btw_sentinel(&format!("{BTW_SENTINEL} ")).is_none());
}

#[test]
fn test_parse_sentinel_trims_whitespace() {
    let req = parse_btw_sentinel(&format!("{BTW_SENTINEL}   spaces around   ")).unwrap();
    assert_eq!(req.question, "spaces around");
}
