use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn test_parse_setting_value_types_json_scalars() {
    // The shell only ever hands us text, but settings are typed. `set
    // permissions.disable_bypass_mode true` has to become a bool: as a string it
    // deserializes to nothing useful and the setting silently does nothing.
    assert_eq!(parse_setting_value("true"), json!(true));
    assert_eq!(parse_setting_value("false"), json!(false));
    assert_eq!(parse_setting_value("42"), json!(42));
    assert_eq!(parse_setting_value("1.5"), json!(1.5));
}

#[test]
fn test_parse_setting_value_keeps_bare_strings() {
    // The most common set: a model id. Not valid JSON on its own, so the
    // fallback is what makes `coco config set models.main <id>` work at all.
    assert_eq!(
        parse_setting_value("deepseek-openai/deepseek-v4-flash"),
        json!("deepseek-openai/deepseek-v4-flash")
    );
    assert_eq!(parse_setting_value("dark"), json!("dark"));
}

#[test]
fn test_parse_setting_value_handles_json_structures() {
    assert_eq!(parse_setting_value(r#"["a","b"]"#), json!(["a", "b"]));
    assert_eq!(parse_setting_value(r#"{"main":"x"}"#), json!({"main": "x"}));
}

#[test]
fn test_parse_setting_value_quoting_forces_a_string() {
    // The escape hatch for a string that looks like JSON.
    assert_eq!(parse_setting_value(r#""true""#), json!("true"));
}
