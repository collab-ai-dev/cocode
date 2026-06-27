use super::*;
use pretty_assertions::assert_eq;

#[test]
fn positive_tokens_accepts_positive_i64() {
    let tokens = PositiveTokens::try_from(200_000_i64).unwrap();
    assert_eq!(tokens.get(), 200_000);
}

#[test]
fn positive_tokens_rejects_zero() {
    let err = PositiveTokens::try_from(0_i64).unwrap_err();
    matches!(err, ConfigError::NonPositiveTokens { value: 0 });
}

#[test]
fn positive_tokens_rejects_negative() {
    let err = PositiveTokens::try_from(-1_i64).unwrap_err();
    matches!(err, ConfigError::NonPositiveTokens { value: -1 });
}

#[test]
fn positive_tokens_rejects_overflow() {
    let err = PositiveTokens::try_from(i64::from(u32::MAX) + 1).unwrap_err();
    matches!(err, ConfigError::NonPositiveTokens { .. });
}

#[test]
fn positive_tokens_into_u64_is_infallible() {
    let tokens = PositiveTokens::try_from(64_000_i64).unwrap();
    let as_u64: u64 = tokens.into();
    assert_eq!(as_u64, 64_000);
}

#[test]
fn deserialise_rejects_zero() {
    let result: Result<PositiveTokens, _> = serde_json::from_str("0");
    assert!(result.is_err());
}

#[test]
fn deserialise_rejects_negative() {
    let result: Result<PositiveTokens, _> = serde_json::from_str("-1");
    assert!(result.is_err());
}

#[test]
fn deserialise_accepts_positive() {
    let value: PositiveTokens = serde_json::from_str("128000").unwrap();
    assert_eq!(value.get(), 128_000);
}

#[test]
fn positive_count_round_trip() {
    let count = PositiveCount::try_from(40_i64).unwrap();
    let as_u64: u64 = count.into();
    assert_eq!(as_u64, 40);
}
