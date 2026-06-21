use super::*;
use serde_json::json;

#[test]
fn returns_none_for_empty_steps() {
    let result = forward_anthropic_container_id_from_last_step(&[]);
    assert!(result.is_none());
}

#[test]
fn returns_none_when_no_container_id() {
    let steps = vec![StepMetadata {
        provider_metadata: Some(json!({"anthropic": {"other": "value"}})),
    }];
    let result = forward_anthropic_container_id_from_last_step(&steps);
    assert!(result.is_none());
}

#[test]
fn finds_container_id_from_last_step() {
    let steps = vec![
        StepMetadata {
            provider_metadata: Some(json!({
                "anthropic": {"container": {"id": "first-id"}}
            })),
        },
        StepMetadata {
            provider_metadata: Some(json!({
                "anthropic": {"container": {"id": "second-id"}}
            })),
        },
    ];
    let result = forward_anthropic_container_id_from_last_step(&steps).unwrap();
    assert_eq!(
        result.provider_options["anthropic"]["container"]["id"],
        "second-id"
    );
}

#[test]
fn skips_steps_without_container_id() {
    let steps = vec![
        StepMetadata {
            provider_metadata: Some(json!({
                "anthropic": {"container": {"id": "first-id"}}
            })),
        },
        StepMetadata {
            provider_metadata: None,
        },
    ];
    let result = forward_anthropic_container_id_from_last_step(&steps).unwrap();
    assert_eq!(
        result.provider_options["anthropic"]["container"]["id"],
        "first-id"
    );
}
