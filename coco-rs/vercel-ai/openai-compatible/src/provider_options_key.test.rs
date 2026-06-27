use super::*;

#[test]
fn to_camel_case_hyphens() {
    assert_eq!(to_camel_case("my-provider"), "myProvider");
    assert_eq!(to_camel_case("openai-compatible"), "openaiCompatible");
    assert_eq!(to_camel_case("a-b-c"), "aBC");
}

#[test]
fn to_camel_case_underscores() {
    assert_eq!(to_camel_case("my_provider"), "myProvider");
}

#[test]
fn to_camel_case_no_change() {
    assert_eq!(to_camel_case("already"), "already");
    assert_eq!(to_camel_case("myProvider"), "myProvider");
}

#[test]
fn warn_if_raw_key_used() {
    let mut opts = vercel_ai_provider::ProviderOptions::new();
    opts.set(
        "my-provider",
        std::collections::HashMap::from([("temperature".into(), serde_json::json!(0.5))]),
    );
    let mut warnings = vec![];
    warn_if_deprecated_provider_options_key("my-provider", Some(&opts), &mut warnings);
    assert_eq!(warnings.len(), 1);
    match &warnings[0] {
        vercel_ai_provider::Warning::Other { message } => {
            assert!(message.contains("myProvider"));
        }
        _ => panic!("expected Warning::Other"),
    }
}

#[test]
fn no_warn_when_camel_key_used() {
    let mut opts = vercel_ai_provider::ProviderOptions::new();
    opts.set(
        "myProvider",
        std::collections::HashMap::from([("temperature".into(), serde_json::json!(0.5))]),
    );
    let mut warnings = vec![];
    warn_if_deprecated_provider_options_key("my-provider", Some(&opts), &mut warnings);
    assert_eq!(warnings.len(), 0);
}

#[test]
fn get_effective_provider_options_prefers_camel_key() {
    let mut opts = vercel_ai_provider::ProviderOptions::new();
    opts.set(
        "my-provider",
        std::collections::HashMap::from([("value".into(), serde_json::json!("raw"))]),
    );
    opts.set(
        "myProvider",
        std::collections::HashMap::from([("value".into(), serde_json::json!("camel"))]),
    );

    let effective = get_effective_provider_options("my-provider", Some(&opts)).expect("options");
    assert_eq!(effective.get("value"), Some(&serde_json::json!("camel")));
}

#[test]
fn get_effective_provider_options_falls_back_to_openai_compatible() {
    let mut opts = vercel_ai_provider::ProviderOptions::new();
    opts.set(
        "openaiCompatible",
        std::collections::HashMap::from([("value".into(), serde_json::json!("fallback"))]),
    );

    let effective = get_effective_provider_options("my-provider", Some(&opts)).expect("options");
    assert_eq!(effective.get("value"), Some(&serde_json::json!("fallback")));
}
