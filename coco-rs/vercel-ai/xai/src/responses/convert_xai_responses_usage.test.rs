use super::*;
use pretty_assertions::assert_eq;

fn usage(
    input: u64,
    output: u64,
    cached: Option<u64>,
    reasoning: Option<u64>,
) -> XaiResponsesUsage {
    XaiResponsesUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        total_tokens: Some(input + output),
        input_tokens_details: cached.map(|c| XaiResponsesInputTokensDetails {
            cached_tokens: Some(c),
        }),
        output_tokens_details: reasoning.map(|r| XaiResponsesOutputTokensDetails {
            reasoning_tokens: Some(r),
        }),
        ..Default::default()
    }
}

#[test]
fn reasoning_is_subtractive_from_output() {
    let u = convert_xai_responses_usage(&usage(10, 20, None, Some(5)));
    assert_eq!(u.output_tokens.total, Some(20));
    // Unlike chat, Responses reasoning tokens are inclusive in output_tokens.
    assert_eq!(u.output_tokens.text, Some(15));
    assert_eq!(u.output_tokens.reasoning, Some(5));
}

#[test]
fn inclusive_cache_branch() {
    // cached <= input → input total is inclusive.
    let u = convert_xai_responses_usage(&usage(100, 5, Some(30), None));
    assert_eq!(u.input_tokens.total(), Some(100));
    assert_eq!(u.input_tokens.no_cache(), Some(70));
    assert_eq!(u.input_tokens.cache_read(), Some(30));
}

#[test]
fn exclusive_cache_branch() {
    // cached > input → reported exclusively, so total = input + cached.
    let u = convert_xai_responses_usage(&usage(40, 5, Some(60), None));
    assert_eq!(u.input_tokens.total(), Some(100));
    assert_eq!(u.input_tokens.no_cache(), Some(40));
    assert_eq!(u.input_tokens.cache_read(), Some(60));
}

#[test]
fn raw_is_preserved() {
    let u = convert_xai_responses_usage(&usage(10, 20, None, None));
    assert!(u.raw.is_some());
}
