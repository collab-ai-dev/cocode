use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn strips_additional_properties_false() {
    let input = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "a": { "type": "string" }
        }
    });
    let cleaned = remove_additional_properties_false(&input);
    assert_eq!(
        cleaned,
        json!({
            "type": "object",
            "properties": { "a": { "type": "string" } }
        })
    );
}

#[test]
fn keeps_additional_properties_true() {
    let input = json!({ "additionalProperties": true });
    assert_eq!(remove_additional_properties_false(&input), input);
}

#[test]
fn recurses_into_nested_objects_and_arrays() {
    let input = json!({
        "type": "object",
        "properties": {
            "nested": {
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }
        },
        "anyOf": [
            { "additionalProperties": false, "type": "object" }
        ]
    });
    let cleaned = remove_additional_properties_false(&input);
    assert!(
        cleaned["properties"]["nested"]
            .get("additionalProperties")
            .is_none()
    );
    assert!(cleaned["anyOf"][0].get("additionalProperties").is_none());
    assert_eq!(cleaned["anyOf"][0]["type"], "object");
}

#[test]
fn leaves_scalars_untouched() {
    assert_eq!(remove_additional_properties_false(&json!(5)), json!(5));
    assert_eq!(remove_additional_properties_false(&json!("x")), json!("x"));
}
