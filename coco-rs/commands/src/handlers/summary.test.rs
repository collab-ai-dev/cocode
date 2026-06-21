use super::*;

#[tokio::test]
async fn handler_emits_sentinel_and_status_text() {
    let out = handler(String::new()).await.unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], SUMMARY_SENTINEL);
    assert!(
        lines[1..]
            .iter()
            .any(|l| l.contains("Extracting session memory"))
    );
}

#[test]
fn parses_sentinel_with_status_text() {
    let body = format!("{SUMMARY_SENTINEL}\nExtracting session memory…\n");
    let parsed = parse_summary_sentinel(&body).expect("should parse");
    assert!(parsed.display_text.contains("Extracting session memory"));
}

#[test]
fn parse_returns_none_for_non_sentinel_text() {
    assert!(parse_summary_sentinel("hello world").is_none());
}
