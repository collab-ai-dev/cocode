use super::*;
use serde_json::json;

#[test]
fn serializes_person_generation_to_snake_case() {
    let opts = GoogleGenerativeAIImageOptions {
        person_generation: Some(PersonGeneration::AllowAdult),
        aspect_ratio: None,
    };
    let v = serde_json::to_value(&opts).unwrap();
    assert_eq!(v, json!({ "personGeneration": "allow_adult" }));
}

#[test]
fn serializes_aspect_ratio_with_colon_format() {
    let opts = GoogleGenerativeAIImageOptions {
        person_generation: None,
        aspect_ratio: Some(AspectRatio::Landscape16x9),
    };
    let v = serde_json::to_value(&opts).unwrap();
    assert_eq!(v, json!({ "aspectRatio": "16:9" }));
}

#[test]
fn deserializes_camel_case_fields() {
    let v = json!({ "personGeneration": "dont_allow", "aspectRatio": "1:1" });
    let opts: GoogleGenerativeAIImageOptions = serde_json::from_value(v).unwrap();
    assert_eq!(opts.person_generation, Some(PersonGeneration::DontAllow));
    assert_eq!(opts.aspect_ratio, Some(AspectRatio::Square));
}

#[test]
fn empty_serializes_to_empty_object() {
    let opts = GoogleGenerativeAIImageOptions::default();
    let v = serde_json::to_value(&opts).unwrap();
    assert_eq!(v, json!({}));
}
