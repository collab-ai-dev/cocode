use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parse_locale_chinese_variants() {
    assert_eq!(parse_locale("zh_CN.UTF-8"), Some("zh-CN"));
    assert_eq!(parse_locale("zh-CN"), Some("zh-CN"));
    assert_eq!(parse_locale("zh"), Some("zh-CN"));
    assert_eq!(parse_locale("zh_Hans"), Some("zh-CN"));
}

#[test]
fn parse_locale_english_variants() {
    assert_eq!(parse_locale("en_US.UTF-8"), Some("en"));
    assert_eq!(parse_locale("en-US"), Some("en"));
    assert_eq!(parse_locale("en"), Some("en"));
}

#[test]
fn parse_locale_unknown_returns_none() {
    assert_eq!(parse_locale("fr_FR.UTF-8"), None);
    assert_eq!(parse_locale("de"), None);
    assert_eq!(parse_locale(""), None);
}

#[test]
fn t_macro_returns_non_empty_for_known_key() {
    let _locale = locale_test_guard("en");
    let text = t!("command.toggle_plan_mode");
    assert!(!text.is_empty());
}

#[test]
fn set_locale_switches_translation() {
    let _locale = locale_test_guard("en");
    set_locale("en");
    let en = t!("command.submit_input").to_string();
    set_locale("zh-CN");
    let zh = t!("command.submit_input").to_string();

    assert_eq!(en, "Submit Input");
    assert_eq!(zh, "提交");
}
