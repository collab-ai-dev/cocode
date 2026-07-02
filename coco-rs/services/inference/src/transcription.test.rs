use super::*;

#[test]
fn auto_language_yields_no_provider_options() {
    assert!(language_provider_options(Some("auto")).is_none());
    assert!(language_provider_options(Some("AUTO")).is_none());
    assert!(language_provider_options(Some("")).is_none());
    assert!(language_provider_options(None).is_none());
}

#[test]
fn concrete_language_sets_openai_namespace() {
    let opts = language_provider_options(Some("es")).expect("options");
    let openai = opts.0.get("openai").expect("openai namespace");
    assert_eq!(
        openai.get("language"),
        Some(&serde_json::Value::String("es".to_string()))
    );
}
