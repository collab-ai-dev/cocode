use super::KeybindingContext;
use std::str::FromStr;

#[test]
fn all_has_20_contexts() {
    // 18 user-rebindable + 2 internal (Scroll, MessageActions).
    assert_eq!(KeybindingContext::ALL.len(), 20);
}

#[test]
fn all_user_has_18_contexts() {
    // 18 entries match the user-facing schema.
    assert_eq!(KeybindingContext::ALL_USER.len(), 18);
    for ctx in KeybindingContext::ALL_USER {
        assert!(
            ctx.is_user_rebindable(),
            "{ctx:?} should be user-rebindable"
        );
    }
}

#[test]
fn internal_contexts_not_user_rebindable() {
    assert!(!KeybindingContext::Scroll.is_user_rebindable());
    assert!(!KeybindingContext::MessageActions.is_user_rebindable());
}

#[test]
fn round_trip_via_string() {
    for ctx in KeybindingContext::ALL {
        let s = ctx.as_str();
        let parsed = KeybindingContext::from_str(s).unwrap();
        assert_eq!(parsed, *ctx);
    }
}

#[test]
fn rejects_unknown_context() {
    assert!(KeybindingContext::from_str("Bogus").is_err());
    // Wire format is PascalCase; lowercase should be rejected.
    assert!(KeybindingContext::from_str("global").is_err());
}

#[test]
fn serde_round_trip() {
    let ctx = KeybindingContext::DiffDialog;
    let json = serde_json::to_string(&ctx).unwrap();
    assert_eq!(json, "\"DiffDialog\"");
    let parsed: KeybindingContext = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, ctx);
}

#[test]
fn descriptions_are_non_empty() {
    for ctx in KeybindingContext::ALL {
        assert!(!ctx.description().is_empty(), "{ctx:?}");
    }
}
