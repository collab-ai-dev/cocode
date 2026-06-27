use super::*;

#[test]
fn test_context_window_sizes_are_ordered() {
    const _: () = assert!(DEFAULT_CONTEXT_WINDOW < CONTEXT_WINDOW_1M);
    assert_eq!(DEFAULT_CONTEXT_WINDOW, 200_000);
    assert_eq!(CONTEXT_WINDOW_1M, 1_000_000);
}

// `test_output_token_limits_hierarchy` removed: the global
// max-output-tokens constants it asserted on were deleted
// when the per-model `ModelInfo` design replaced them. See
// `constants.rs` comment block for the rationale.

#[test]
fn test_tool_result_bytes_derived_correctly() {
    assert_eq!(
        MAX_TOOL_RESULT_BYTES,
        MAX_TOOL_RESULT_TOKENS * BYTES_PER_TOKEN
    );
    assert_eq!(MAX_TOOL_RESULT_BYTES, 400_000);
}

#[test]
fn test_image_target_raw_size_under_base64_limit() {
    // raw * 4/3 should not exceed the base64 limit
    let base64_size = IMAGE_TARGET_RAW_SIZE * 4 / 3;
    assert!(base64_size <= API_IMAGE_MAX_BASE64_SIZE);
}

#[test]
fn test_api_timeout_is_reasonable() {
    const _: () = assert!(DEFAULT_API_TIMEOUT_SECS >= 60);
    const _: () = assert!(DEFAULT_API_TIMEOUT_SECS <= 1800);
}

#[test]
fn test_product_constants_not_empty() {
    assert!(!PRODUCT_NAME.is_empty());
    assert!(!CONFIG_DIR_NAME.is_empty());
    assert!(!PROJECT_CONFIG_DIR.is_empty());
    assert!(!PRODUCT_URL.is_empty());
    assert!(PRODUCT_URL.starts_with("https://"));
}

#[test]
fn test_api_urls_are_https() {
    assert!(ANTHROPIC_API_BASE_URL.starts_with("https://"));
    assert!(CLAUDE_AI_BASE_URL.starts_with("https://"));
    assert!(OAUTH_TOKEN_URL.starts_with("https://"));
    assert!(OAUTH_AUTHORIZE_URL.starts_with("https://"));
}

#[test]
fn test_auto_compact_threshold_is_percentage() {
    const _: () = assert!(DEFAULT_AUTO_COMPACT_PCT > 0);
    const _: () = assert!(DEFAULT_AUTO_COMPACT_PCT <= 100);
}

#[test]
fn test_pdf_limits_ordering() {
    const _: () = assert!(PDF_EXTRACT_SIZE_THRESHOLD < PDF_TARGET_RAW_SIZE);
    const _: () = assert!(PDF_TARGET_RAW_SIZE < PDF_MAX_EXTRACT_SIZE);
}

#[test]
fn test_max_media_per_request_positive() {
    const _: () = assert!(API_MAX_MEDIA_PER_REQUEST > 0);
}
