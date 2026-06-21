use super::*;

#[test]
fn partial_default_is_empty() {
    // Pin the invariant `PartialModelInfo::default().is_empty()`.
    // If a future field gains a non-None `Default`, this test fails
    // and forces a re-think of the round-trip semantics.
    assert!(PartialModelInfo::default().is_empty());
}

#[test]
fn merge_from_collapses_some_empty_extra_body_to_none() {
    // Round-trip stability: an explicitly-empty extra_body overlay
    // does not change wire-format presence.
    let mut acc = PartialModelInfo::default();
    let overlay = PartialModelInfo {
        extra_body: Some(std::collections::BTreeMap::new()),
        ..Default::default()
    };
    acc.merge_from(&overlay);
    assert!(acc.extra_body.is_none());
}

#[test]
fn models_json_round_trip_is_byte_stable() {
    // Plan §15 Group B claim #7: BTreeMap on disk produces stable
    // serialisation. Round-trip 100x and assert byte-identical.
    use crate::positive::PositiveTokens;
    use std::collections::BTreeMap;

    let mut catalog: BTreeMap<String, PartialModelInfo> = BTreeMap::new();
    catalog.insert(
        "claude-opus-4-7".into(),
        PartialModelInfo {
            context_window: Some(PositiveTokens::new(200_000)),
            max_output_tokens: Some(PositiveTokens::new(64_000)),
            extra_body: Some(
                [
                    ("cacheControl".to_string(), serde_json::json!("ephemeral")),
                    ("zLast".to_string(), serde_json::json!(true)),
                    ("aFirst".to_string(), serde_json::json!(false)),
                ]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        },
    );
    catalog.insert(
        "gpt-5".into(),
        PartialModelInfo {
            context_window: Some(PositiveTokens::new(272_000)),
            max_output_tokens: Some(PositiveTokens::new(16_384)),
            ..Default::default()
        },
    );

    let mut current = serde_json::to_string_pretty(&catalog).unwrap();
    for _ in 0..100 {
        let parsed: BTreeMap<String, PartialModelInfo> = serde_json::from_str(&current).unwrap();
        let next = serde_json::to_string_pretty(&parsed).unwrap();
        assert_eq!(current, next, "models.json must be byte-stable");
        current = next;
    }
}

#[test]
fn shell_tool_type_parses_valid_values() {
    let parsed: PartialModelInfo = serde_json::from_value(serde_json::json!({
        "shell_tool_type": "unified_exec"
    }))
    .expect("valid shell_tool_type should parse");
    assert_eq!(
        parsed.shell_tool_type,
        Some(coco_types::ModelShellToolType::UnifiedExec)
    );
}

#[test]
fn shell_tool_type_rejects_invalid_values() {
    let err = serde_json::from_value::<PartialModelInfo>(serde_json::json!({
        "shell_tool_type": "fish"
    }))
    .expect_err("invalid shell_tool_type must fail");
    assert!(err.to_string().contains("unknown variant"));
}

#[test]
fn legacy_shell_type_field_is_rejected() {
    let err = serde_json::from_value::<PartialModelInfo>(serde_json::json!({
        "shell_type": "shell_command"
    }))
    .expect_err("legacy shell_type must fail");
    assert!(err.to_string().contains("unknown field"));
}

#[test]
fn merge_from_preserves_existing_extra_body_on_empty_overlay() {
    let mut acc = PartialModelInfo {
        extra_body: Some(std::collections::BTreeMap::from([(
            "store".into(),
            serde_json::Value::Bool(false),
        )])),
        ..Default::default()
    };
    let overlay = PartialModelInfo {
        extra_body: Some(std::collections::BTreeMap::new()),
        ..Default::default()
    };
    acc.merge_from(&overlay);
    assert_eq!(
        acc.extra_body
            .as_ref()
            .and_then(|m| m.get("store"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
}
