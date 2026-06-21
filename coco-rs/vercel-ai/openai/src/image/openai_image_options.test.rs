use super::*;

#[test]
fn max_images_known_models() {
    assert_eq!(model_max_images_per_call("dall-e-2"), 10);
    assert_eq!(model_max_images_per_call("gpt-image-1"), 10);
    assert_eq!(model_max_images_per_call("gpt-image-1-mini"), 10);
    assert_eq!(model_max_images_per_call("gpt-image-1.5"), 10);
    assert_eq!(model_max_images_per_call("chatgpt-image-latest"), 10);
}

#[test]
fn max_images_dall_e_3() {
    assert_eq!(model_max_images_per_call("dall-e-3"), 1);
}

#[test]
fn max_images_unknown_defaults_to_one() {
    assert_eq!(model_max_images_per_call("unknown-model"), 1);
    assert_eq!(model_max_images_per_call("some-future-model"), 1);
}

#[test]
fn has_default_response_format_known() {
    assert!(has_default_response_format("chatgpt-image-foo"));
    assert!(has_default_response_format("gpt-image-1"));
    assert!(has_default_response_format("gpt-image-1-mini"));
    assert!(has_default_response_format("gpt-image-1.5"));
}

#[test]
fn has_default_response_format_dall_e() {
    assert!(!has_default_response_format("dall-e-2"));
    assert!(!has_default_response_format("dall-e-3"));
}
