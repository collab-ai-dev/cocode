use super::*;
use pretty_assertions::assert_eq;

#[test]
fn builds_browser_search_tool() {
    let tool = browser_search();
    assert_eq!(tool.id, BROWSER_SEARCH_TOOL_ID);
    assert_eq!(tool.id, "groq.browser_search");
    assert_eq!(tool.name, "browser_search");
    assert!(tool.args.is_empty());
}
