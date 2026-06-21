use super::*;

#[tokio::test]
async fn test_compact_handler_no_args_emits_sentinel() {
    let output = handler(String::new()).await.unwrap();
    assert!(output.starts_with(COMPACT_SENTINEL));
    assert!(output.contains("Compacting conversation"));
    assert!(output.contains("compact representation"));
}

#[tokio::test]
async fn test_compact_handler_with_instructions_passes_them_in_sentinel() {
    let output = handler("focus on API changes".to_string()).await.unwrap();
    let first_line = output.lines().next().unwrap();
    assert!(first_line.starts_with(COMPACT_SENTINEL));
    assert!(first_line.contains("focus on API changes"));
    assert!(output.contains("Summarization focus: focus on API changes"));
}

#[tokio::test]
async fn test_compact_handler_no_args_has_empty_instructions() {
    let output = handler(String::new()).await.unwrap();
    let first_line = output.lines().next().unwrap();
    // Sentinel + space + (no instructions)
    assert_eq!(first_line.trim_end(), COMPACT_SENTINEL);
}

#[tokio::test]
async fn test_compact_handler_trims_whitespace_in_args() {
    let output = handler("   spaces around   ".to_string()).await.unwrap();
    let first_line = output.lines().next().unwrap();
    assert!(first_line.contains("spaces around"));
    assert!(!first_line.contains("   spaces"));
}

#[tokio::test]
async fn test_parse_compact_sentinel_with_instructions() {
    let output = handler("focus on auth".to_string()).await.unwrap();
    let req = parse_compact_sentinel(&output).expect("sentinel must parse");
    assert_eq!(req.custom_instructions, "focus on auth");
    assert!(req.display_text.contains("Compacting conversation"));
}

#[tokio::test]
async fn test_parse_compact_sentinel_no_args() {
    let output = handler(String::new()).await.unwrap();
    let req = parse_compact_sentinel(&output).expect("sentinel must parse");
    assert!(req.custom_instructions.is_empty());
}

#[test]
fn test_parse_compact_sentinel_returns_none_for_plain_text() {
    assert!(parse_compact_sentinel("just a normal command output").is_none());
}
