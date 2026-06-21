use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_detect_gateway_from_headers() {
    let headers = vec!["content-type", "x-litellm-model-id", "x-litellm-key-alias"];
    assert_eq!(detect_gateway(&headers, None), Some(KnownGateway::Litellm));

    let headers = vec!["helicone-id", "content-length"];
    assert_eq!(detect_gateway(&headers, None), Some(KnownGateway::Helicone));

    let headers = vec!["content-type", "x-request-id"];
    assert_eq!(detect_gateway(&headers, None), None);
}

#[test]
fn test_detect_gateway_from_url() {
    assert_eq!(
        detect_gateway(
            &[],
            Some("https://my-workspace.cloud.databricks.com/serving-endpoints/chat")
        ),
        Some(KnownGateway::Databricks)
    );
    assert_eq!(
        detect_gateway(&[], Some("https://api.anthropic.com/v1/messages")),
        None,
    );
    // Malformed URL should not panic
    assert_eq!(detect_gateway(&[], Some("not-a-url")), None);
}

#[test]
fn test_detect_gateway_headers_take_priority() {
    // If headers match one gateway and URL matches another, headers win
    let headers = vec!["x-portkey-request-id"];
    assert_eq!(
        detect_gateway(&headers, Some("https://my.cloud.databricks.com/foo")),
        Some(KnownGateway::Portkey)
    );
}

#[test]
fn test_format_request_log_basic() {
    let log = RequestLog {
        model: "claude-sonnet-4-20250514".into(),
        message_count: 12,
        input_tokens_estimate: Some(5000),
        temperature: 0.7,
        provider: "anthropic".into(),
        query_source: "repl_main_thread".into(),
        fast_mode: false,
        thinking_type: None,
        effort_value: None,
    };
    let formatted = format_request_log(&log);
    assert!(formatted.contains("model=claude-sonnet-4-20250514"));
    assert!(formatted.contains("msgs=12"));
    assert!(formatted.contains("~5000tok"));
    assert!(formatted.contains("temp=0.7"));
    assert!(!formatted.contains("[fast]"));
}

#[test]
fn test_format_request_log_fast_mode() {
    let log = RequestLog {
        model: "claude-haiku".into(),
        message_count: 1,
        input_tokens_estimate: None,
        temperature: 0.0,
        provider: "anthropic".into(),
        query_source: "sdk".into(),
        fast_mode: true,
        thinking_type: Some("adaptive".into()),
        effort_value: None,
    };
    let formatted = format_request_log(&log);
    assert!(formatted.contains("[fast]"));
    assert!(formatted.contains("thinking=adaptive"));
    assert!(!formatted.contains("~"));
}

#[test]
fn test_format_response_log() {
    let log = ResponseLog {
        model: "claude-sonnet-4-20250514".into(),
        usage: TokenUsage {
            input_tokens: coco_types::InputTokens {
                total: 10000,
                cache_read: 8000,
                cache_write: 2000,
                ..Default::default()
            },
            output_tokens: coco_types::OutputTokens {
                total: 500,
                ..Default::default()
            },
        },
        duration_ms: 1200,
        duration_ms_including_retries: 1200,
        ttft_ms: Some(350),
        attempt: 1,
        request_id: Some("req_123".into()),
        stop_reason: Some(StopReason::EndTurn),
        cost_usd: 0.0042,
        did_fallback_to_non_streaming: false,
        gateway: None,
        message_count: 5,
        text_content_length: Some(1500),
        thinking_content_length: None,
        fast_mode: false,
        warnings: vec![],
    };
    let formatted = format_response_log(&log);
    assert!(formatted.contains("in=10000tok"));
    assert!(formatted.contains("out=500tok"));
    assert!(formatted.contains("cache=80%"));
    assert!(formatted.contains("ttft=350ms"));
    assert!(formatted.contains("stop=end_turn"));
    assert!(!formatted.contains("retries="));
}

#[test]
fn test_format_response_log_with_retries_and_gateway() {
    let log = ResponseLog {
        model: "claude-sonnet-4-20250514".into(),
        usage: TokenUsage {
            input_tokens: coco_types::InputTokens {
                total: 100,
                ..Default::default()
            },
            output_tokens: coco_types::OutputTokens {
                total: 50,
                ..Default::default()
            },
        },
        duration_ms: 3000,
        duration_ms_including_retries: 9000,
        ttft_ms: None,
        attempt: 3,
        request_id: None,
        stop_reason: None,
        cost_usd: 0.001,
        did_fallback_to_non_streaming: true,
        gateway: Some(KnownGateway::Litellm),
        message_count: 2,
        text_content_length: None,
        thinking_content_length: None,
        fast_mode: false,
        warnings: vec!["slow response".into()],
    };
    let formatted = format_response_log(&log);
    assert!(formatted.contains("retries=2"));
    assert!(formatted.contains("via=litellm"));
    assert!(formatted.contains("warnings=[slow response]"));
}

#[test]
fn test_parse_api_error_message_nested() {
    let body = r#"{"error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}}"#;
    assert_eq!(
        parse_api_error_message(body),
        Some("Rate limit exceeded".into())
    );
}

#[test]
fn test_parse_api_error_message_flat() {
    let body = r#"{"message": "Server error", "type": "server_error"}"#;
    assert_eq!(parse_api_error_message(body), Some("Server error".into()));
}

#[test]
fn test_parse_api_error_message_invalid_json() {
    assert_eq!(parse_api_error_message("not json"), None);
    assert_eq!(parse_api_error_message("{}"), None);
}

#[test]
fn test_parse_prompt_too_long_tokens() {
    assert_eq!(
        parse_prompt_too_long_tokens("prompt is too long: 137500 tokens > 135000 maximum"),
        Some((137500, 135000))
    );
    assert_eq!(
        parse_prompt_too_long_tokens("Prompt is too long — 200000 tokens > 128000"),
        Some((200000, 128000))
    );
    assert_eq!(
        parse_prompt_too_long_tokens("some random error message"),
        None,
    );
}

#[test]
fn test_error_log_from_inference_error() {
    let error = crate::errors::RateLimitedSnafu {
        retry_after_ms: Some(5000_i64),
        message: "too many requests".to_string(),
    }
    .build();
    let log = ErrorLog::from_inference_error(
        &error,
        "claude-sonnet-4-20250514",
        /*message_count*/ 10,
        /*duration_ms*/ 500,
        /*duration_ms_including_retries*/ 1500,
        /*attempt*/ 2,
        Some("req_456".into()),
    );
    assert_eq!(log.error_class, "rate_limit");
    assert_eq!(log.status, Some(429));
    assert_eq!(log.attempt, 2);
    assert_eq!(log.model, "claude-sonnet-4-20250514");
}

#[test]
fn test_response_log_to_properties() {
    let log = ResponseLog {
        model: "test-model".into(),
        usage: TokenUsage {
            input_tokens: coco_types::InputTokens {
                total: 100,
                cache_read: 80,
                cache_write: 20,
                ..Default::default()
            },
            output_tokens: coco_types::OutputTokens {
                total: 50,
                ..Default::default()
            },
        },
        duration_ms: 500,
        duration_ms_including_retries: 500,
        ttft_ms: Some(100),
        attempt: 1,
        request_id: Some("req_abc".into()),
        stop_reason: Some(StopReason::EndTurn),
        cost_usd: 0.001,
        did_fallback_to_non_streaming: false,
        gateway: None,
        message_count: 3,
        text_content_length: None,
        thinking_content_length: None,
        fast_mode: true,
        warnings: vec![],
    };
    let props = response_log_to_properties(&log);
    assert_eq!(props["model"], serde_json::json!("test-model"));
    assert_eq!(props["input_tokens"], serde_json::json!(100));
    assert_eq!(props["cache_read_tokens"], serde_json::json!(80));
    assert_eq!(props["fast_mode"], serde_json::json!(true));
    assert_eq!(props["ttft_ms"], serde_json::json!(100));
    assert_eq!(props["stop_reason"], serde_json::json!("end_turn"));
    assert_eq!(props["request_id"], serde_json::json!("req_abc"));
}

#[test]
fn test_duration_to_ms() {
    assert_eq!(duration_to_ms(Duration::from_millis(1500)), 1500);
    assert_eq!(duration_to_ms(Duration::from_secs(2)), 2000);
    assert_eq!(duration_to_ms(Duration::ZERO), 0);
}

#[test]
fn test_stop_reason_display() {
    assert_eq!(StopReason::EndTurn.to_string(), "end_turn");
    assert_eq!(StopReason::MaxTokens.to_string(), "max_tokens");
    assert_eq!(StopReason::ToolUse.to_string(), "tool_use");
}

#[test]
fn test_known_gateway_display() {
    assert_eq!(KnownGateway::Litellm.to_string(), "litellm");
    assert_eq!(
        KnownGateway::CloudflareAiGateway.to_string(),
        "cloudflare-ai-gateway"
    );
}
