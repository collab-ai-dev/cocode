use super::*;

#[test]
fn test_extract_reasoning_from_text() {
    let middleware = ExtractReasoningMiddleware::new("<think>", "</think>");

    let text = "Hello <think>this is reasoning</think> world";
    let result = middleware.extract_reasoning_from_text(text);

    assert_eq!(result.len(), 3);
    assert_eq!(result[0], (false, "Hello ".to_string()));
    assert_eq!(result[1], (true, "this is reasoning".to_string()));
    assert_eq!(result[2], (false, " world".to_string()));
}

#[test]
fn test_extract_multiple_reasoning_blocks() {
    let middleware = ExtractReasoningMiddleware::new("<think>", "</think>");

    let text = "Start <think>first</think> middle <think>second</think> end";
    let result = middleware.extract_reasoning_from_text(text);

    assert_eq!(result.len(), 5);
    assert_eq!(result[0], (false, "Start ".to_string()));
    assert_eq!(result[1], (true, "first".to_string()));
    assert_eq!(result[2], (false, " middle ".to_string()));
    assert_eq!(result[3], (true, "second".to_string()));
    assert_eq!(result[4], (false, " end".to_string()));
}

#[test]
fn test_no_reasoning_tags() {
    let middleware = ExtractReasoningMiddleware::new("<think>", "</think>");

    let text = "Just regular text";
    let result = middleware.extract_reasoning_from_text(text);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], (false, "Just regular text".to_string()));
}
