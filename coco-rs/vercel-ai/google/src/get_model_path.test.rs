use super::*;

#[test]
fn prepends_models_prefix_for_simple_id() {
    assert_eq!(
        get_model_path("gemini-2.0-flash"),
        "models/gemini-2.0-flash"
    );
}

#[test]
fn returns_as_is_when_contains_slash() {
    assert_eq!(
        get_model_path("publishers/google/models/gemini-2.0"),
        "publishers/google/models/gemini-2.0"
    );
}

#[test]
fn handles_empty_string() {
    assert_eq!(get_model_path(""), "models/");
}
