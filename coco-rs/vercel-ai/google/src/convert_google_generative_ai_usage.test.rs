use super::*;

#[test]
fn converts_full_usage() {
    let google_usage = GoogleUsageMetadata {
        prompt_token_count: Some(100),
        candidates_token_count: Some(200),
        cached_content_token_count: Some(50),
        thoughts_token_count: Some(30),
        total_token_count: Some(300),
        traffic_type: None,
        ..Default::default()
    };
    let usage = convert_usage(Some(&google_usage));
    assert_eq!(usage.total_input_tokens(), 100);
    // output total = candidates + thoughts = 200 + 30 = 230
    assert_eq!(usage.output_tokens.total, Some(230));
    assert_eq!(usage.output_tokens.text, Some(200));
    assert_eq!(usage.input_tokens.cache_read(), Some(50));
    // no_cache = prompt - cached = 100 - 50 = 50
    assert_eq!(usage.input_tokens.no_cache(), Some(50));
    assert_eq!(usage.output_tokens.reasoning, Some(30));
    assert!(usage.raw.is_some());
}

#[test]
fn handles_none_usage() {
    let usage = convert_usage(None);
    assert_eq!(usage.total_input_tokens(), 0);
    assert_eq!(usage.total_output_tokens(), 0);
}

#[test]
fn handles_partial_usage() {
    let google_usage = GoogleUsageMetadata {
        prompt_token_count: Some(100),
        candidates_token_count: None,
        cached_content_token_count: None,
        thoughts_token_count: None,
        total_token_count: None,
        traffic_type: None,
        ..Default::default()
    };
    let usage = convert_usage(Some(&google_usage));
    assert_eq!(usage.total_input_tokens(), 100);
    assert_eq!(usage.total_output_tokens(), 0);
    assert_eq!(usage.input_tokens.no_cache(), Some(100));
    assert_eq!(usage.output_tokens.text, Some(0));
}

#[test]
fn includes_traffic_type_in_raw() {
    let google_usage = GoogleUsageMetadata {
        prompt_token_count: Some(10),
        candidates_token_count: Some(20),
        cached_content_token_count: None,
        thoughts_token_count: None,
        total_token_count: Some(30),
        traffic_type: Some("ON_DEMAND".to_string()),
        ..Default::default()
    };
    let usage = convert_usage(Some(&google_usage));
    let raw = usage.raw.unwrap();
    assert_eq!(raw["trafficType"], "ON_DEMAND");
}
