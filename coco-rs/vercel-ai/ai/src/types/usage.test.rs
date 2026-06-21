use super::*;
use vercel_ai_provider::ImageModelV4Usage;
use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

#[test]
fn test_as_language_model_usage() {
    let usage = Usage {
        input_tokens: InputTokens::from_exclusive_buckets(
            /*no_cache*/ Some(80),
            /*cache_read*/ Some(15),
            /*cache_write*/ Some(5),
        ),
        output_tokens: OutputTokens {
            total: Some(50),
            text: Some(40),
            reasoning: Some(10),
        },
        raw: None,
    };

    let lm_usage = as_language_model_usage(&usage);
    assert_eq!(lm_usage.input_tokens, Some(100));
    assert_eq!(lm_usage.output_tokens, Some(50));
    assert_eq!(lm_usage.total_tokens, Some(150));
    assert_eq!(lm_usage.input_token_details.no_cache_tokens, Some(80));
    assert_eq!(lm_usage.input_token_details.cache_read_tokens, Some(15));
    assert_eq!(lm_usage.input_token_details.cache_write_tokens, Some(5));
    assert_eq!(lm_usage.output_token_details.text_tokens, Some(40));
    assert_eq!(lm_usage.output_token_details.reasoning_tokens, Some(10));
    assert!(lm_usage.raw.is_none());
}

#[test]
fn test_as_language_model_usage_preserves_none() {
    let usage = Usage {
        input_tokens: InputTokens::default(),
        output_tokens: OutputTokens::default(),
        raw: None,
    };

    let lm_usage = as_language_model_usage(&usage);
    assert_eq!(lm_usage.input_tokens, None);
    assert_eq!(lm_usage.output_tokens, None);
    assert_eq!(lm_usage.total_tokens, None);
}

#[test]
fn test_as_language_model_usage_forwards_raw() {
    let mut raw = HashMap::new();
    raw.insert("custom".to_string(), serde_json::json!(42));

    let usage = Usage {
        input_tokens: InputTokens::from_total(Some(10)),
        output_tokens: OutputTokens {
            total: Some(5),
            ..Default::default()
        },
        raw: Some(raw.clone()),
    };

    let lm_usage = as_language_model_usage(&usage);
    assert_eq!(lm_usage.raw.unwrap()["custom"], serde_json::json!(42));
}

#[test]
fn test_add_language_model_usage() {
    let a = LanguageModelUsage {
        input_tokens: Some(100),
        output_tokens: Some(50),
        total_tokens: Some(150),
        ..Default::default()
    };
    let b = LanguageModelUsage {
        input_tokens: Some(200),
        output_tokens: Some(80),
        total_tokens: Some(280),
        ..Default::default()
    };

    let sum = add_language_model_usage(&a, &b);
    assert_eq!(sum.input_tokens, Some(300));
    assert_eq!(sum.output_tokens, Some(130));
    assert_eq!(sum.total_tokens, Some(430));
}

#[test]
fn test_add_language_model_usage_with_none() {
    let a = LanguageModelUsage {
        input_tokens: Some(100),
        output_tokens: None,
        ..Default::default()
    };
    let b = LanguageModelUsage {
        input_tokens: None,
        output_tokens: Some(80),
        ..Default::default()
    };

    let sum = add_language_model_usage(&a, &b);
    assert_eq!(sum.input_tokens, Some(100));
    assert_eq!(sum.output_tokens, Some(80));
}

#[test]
fn test_create_null_usage() {
    let usage = create_null_language_model_usage();
    assert!(usage.input_tokens.is_none());
    assert!(usage.output_tokens.is_none());
    assert!(usage.total_tokens.is_none());
}

#[test]
fn test_add_image_model_usage() {
    let a = ImageModelV4Usage {
        prompt_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
    };
    let b = ImageModelV4Usage {
        prompt_tokens: 200,
        output_tokens: 100,
        total_tokens: 300,
    };

    let sum = add_image_model_usage(&a, &b);
    assert_eq!(sum.prompt_tokens, 300);
    assert_eq!(sum.output_tokens, 150);
    assert_eq!(sum.total_tokens, 450);
}
