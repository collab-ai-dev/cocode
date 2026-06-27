use pretty_assertions::assert_eq;

use super::enabled_from;

#[test]
fn test_enabled_from_truthy_values() {
    for v in ["1", "true", "TRUE", "yes", "on", "  1  "] {
        assert_eq!(enabled_from(Some(v)), true, "{v:?} should enable");
    }
}

#[test]
fn test_enabled_from_falsey_values() {
    for v in [
        None,
        Some(""),
        Some("   "),
        Some("0"),
        Some("false"),
        Some("False"),
    ] {
        assert_eq!(enabled_from(v), false, "{v:?} should not enable");
    }
}
