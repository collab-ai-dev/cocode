use super::*;
use crate::event::RateLimitStatus;
use pretty_assertions::assert_eq;

#[test]
fn rate_limit_entry_serde_roundtrip() {
    let entry = RateLimitEntry {
        api: ProviderApi::Anthropic,
        status: RateLimitStatus::Rejected,
        reset_at_ms: Some(1_700_000_000_000),
        retry_after_seconds: Some(60),
        last_observed_ms: 1_699_999_940_000,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: RateLimitEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(entry, back);
}

#[test]
fn rate_limit_entry_skips_none_fields() {
    // `reset_at_ms` and `retry_after_seconds` are `#[serde(default,
    // skip_serializing_if)]` so an `Allowed` entry with no reset
    // info round-trips compactly.
    let entry = RateLimitEntry {
        api: ProviderApi::Openai,
        status: RateLimitStatus::Allowed,
        reset_at_ms: None,
        retry_after_seconds: None,
        last_observed_ms: 0,
    };
    let json = serde_json::to_string(&entry).unwrap();
    assert!(!json.contains("reset_at_ms"));
    assert!(!json.contains("retry_after_seconds"));
}
