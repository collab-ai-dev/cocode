use super::*;

#[tokio::test]
async fn handler_emits_sentinel_and_status_text() {
    let out = handler(String::new()).await.unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], DREAM_SENTINEL);
    assert!(
        lines[1..]
            .iter()
            .any(|l| l.contains("Consolidating memory"))
    );
}

#[test]
fn parses_sentinel_with_status_text() {
    let body = format!("{DREAM_SENTINEL}\nConsolidating memory…\n");
    let parsed = parse_dream_sentinel(&body).expect("should parse");
    assert!(parsed.display_text.contains("Consolidating memory"));
}

#[test]
fn parse_returns_none_for_non_sentinel_text() {
    assert!(parse_dream_sentinel("hello world").is_none());
}
