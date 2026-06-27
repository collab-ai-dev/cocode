use std::collections::HashMap;

use pretty_assertions::assert_eq;

use super::*;

// ── ElicitationType ──

#[test]
fn test_elicitation_type_serialize_form() {
    let json = serde_json::to_string(&ElicitationType::Form).expect("serialize");
    assert_eq!(json, r#""form""#);
}

#[test]
fn test_elicitation_type_serialize_url() {
    let json = serde_json::to_string(&ElicitationType::Url).expect("serialize");
    assert_eq!(json, r#""url""#);
}

#[test]
fn test_elicitation_type_roundtrip() {
    for variant in [ElicitationType::Form, ElicitationType::Url] {
        let json = serde_json::to_string(&variant).expect("serialize");
        let back: ElicitationType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, variant);
    }
}

// ── ElicitationFieldType ──

#[test]
fn test_field_type_text_roundtrip() {
    let ft = ElicitationFieldType::Text;
    let json = serde_json::to_value(&ft).expect("serialize");
    assert_eq!(json, serde_json::json!({"type": "text"}));
    let back: ElicitationFieldType = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, ft);
}

#[test]
fn test_field_type_number_roundtrip() {
    let ft = ElicitationFieldType::Number;
    let json = serde_json::to_value(&ft).expect("serialize");
    assert_eq!(json, serde_json::json!({"type": "number"}));
    let back: ElicitationFieldType = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, ft);
}

#[test]
fn test_field_type_boolean_roundtrip() {
    let ft = ElicitationFieldType::Boolean;
    let json = serde_json::to_value(&ft).expect("serialize");
    assert_eq!(json, serde_json::json!({"type": "boolean"}));
    let back: ElicitationFieldType = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, ft);
}

#[test]
fn test_field_type_select_roundtrip() {
    let ft = ElicitationFieldType::Select {
        options: vec!["a".to_string(), "b".to_string(), "c".to_string()],
    };
    let json = serde_json::to_value(&ft).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({"type": "select", "options": ["a", "b", "c"]})
    );
    let back: ElicitationFieldType = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, ft);
}

// ── ElicitationField ──

#[test]
fn test_elicitation_field_minimal() {
    let field = ElicitationField {
        name: "username".to_string(),
        field_type: ElicitationFieldType::Text,
        label: None,
        required: false,
        default_value: None,
    };
    let json = serde_json::to_value(&field).expect("serialize");
    // Optional fields with None should be omitted
    assert!(json.get("label").is_none());
    assert!(json.get("default_value").is_none());
}

#[test]
fn test_elicitation_field_full() {
    let json = serde_json::json!({
        "name": "count",
        "field_type": {"type": "number"},
        "label": "Item count",
        "required": true,
        "default_value": 10
    });
    let field: ElicitationField = serde_json::from_value(json).expect("deserialize");
    assert_eq!(field.name, "count");
    assert_eq!(field.field_type, ElicitationFieldType::Number);
    assert_eq!(field.label.as_deref(), Some("Item count"));
    assert!(field.required);
    assert_eq!(field.default_value, Some(serde_json::json!(10)));
}

// ── ElicitationRequest ──

#[test]
fn test_elicitation_request_form_roundtrip() {
    let request = ElicitationRequest {
        server_name: "my-server".to_string(),
        request_id: "req-001".to_string(),
        elicitation_type: ElicitationType::Form,
        title: Some("Enter credentials".to_string()),
        description: Some("Please fill in the fields below".to_string()),
        fields: vec![
            ElicitationField {
                name: "username".to_string(),
                field_type: ElicitationFieldType::Text,
                label: Some("Username".to_string()),
                required: true,
                default_value: None,
            },
            ElicitationField {
                name: "remember".to_string(),
                field_type: ElicitationFieldType::Boolean,
                label: Some("Remember me".to_string()),
                required: false,
                default_value: Some(serde_json::json!(false)),
            },
        ],
        url: None,
        message: None,
    };

    let json = serde_json::to_value(&request).expect("serialize");
    let back: ElicitationRequest = serde_json::from_value(json).expect("deserialize");

    assert_eq!(back.server_name, "my-server");
    assert_eq!(back.request_id, "req-001");
    assert_eq!(back.elicitation_type, ElicitationType::Form);
    assert_eq!(back.title.as_deref(), Some("Enter credentials"));
    assert_eq!(back.fields.len(), 2);
    assert_eq!(back.fields[0].name, "username");
    assert!(back.fields[0].required);
    assert!(back.url.is_none());
}

#[test]
fn test_elicitation_request_url_roundtrip() {
    let request = ElicitationRequest {
        server_name: "oauth-server".to_string(),
        request_id: "req-002".to_string(),
        elicitation_type: ElicitationType::Url,
        title: None,
        description: Some("Visit this URL to authorize".to_string()),
        fields: vec![],
        url: Some("https://auth.example.com/consent?state=abc".to_string()),
        message: None,
    };

    let json = serde_json::to_value(&request).expect("serialize");
    let back: ElicitationRequest = serde_json::from_value(json).expect("deserialize");

    assert_eq!(back.elicitation_type, ElicitationType::Url);
    assert_eq!(
        back.url.as_deref(),
        Some("https://auth.example.com/consent?state=abc")
    );
    assert!(back.fields.is_empty());
}

// ── ElicitationResult ──

#[test]
fn test_elicitation_result_approved() {
    let mut values = HashMap::new();
    values.insert("username".to_string(), serde_json::json!("alice"));
    values.insert("remember".to_string(), serde_json::json!(true));

    let result = ElicitationResult {
        approved: true,
        values,
    };

    let json = serde_json::to_value(&result).expect("serialize");
    let back: ElicitationResult = serde_json::from_value(json).expect("deserialize");

    assert!(back.approved);
    assert_eq!(back.values.len(), 2);
    assert_eq!(back.values["username"], serde_json::json!("alice"));
    assert_eq!(back.values["remember"], serde_json::json!(true));
}

#[test]
fn test_elicitation_result_rejected() {
    let result = ElicitationResult {
        approved: false,
        values: HashMap::new(),
    };

    let json = serde_json::to_value(&result).expect("serialize");
    let back: ElicitationResult = serde_json::from_value(json).expect("deserialize");

    assert!(!back.approved);
    assert!(back.values.is_empty());
}

// ── Legacy types ──

#[test]
fn test_elicitation_mode_url_roundtrip() {
    let mode = ElicitationMode::Url {
        url: "https://example.com".to_string(),
    };
    let json = serde_json::to_value(&mode).expect("serialize");
    let back: ElicitationMode = serde_json::from_value(json).expect("deserialize");
    assert!(matches!(back, ElicitationMode::Url { url } if url == "https://example.com"));
}

#[test]
fn test_elicit_result_completed_roundtrip() {
    let result = ElicitResult::Completed {
        data: serde_json::json!({"key": "value"}),
    };
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["status"], "completed");
    let back: ElicitResult = serde_json::from_value(json).expect("deserialize");
    assert!(matches!(back, ElicitResult::Completed { .. }));
}

#[test]
fn test_elicit_result_cancelled_roundtrip() {
    let result = ElicitResult::Cancelled;
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["status"], "cancelled");
    let back: ElicitResult = serde_json::from_value(json).expect("deserialize");
    assert!(matches!(back, ElicitResult::Cancelled));
}

#[test]
fn test_elicit_result_timeout_roundtrip() {
    let result = ElicitResult::Timeout;
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["status"], "timeout");
    let back: ElicitResult = serde_json::from_value(json).expect("deserialize");
    assert!(matches!(back, ElicitResult::Timeout));
}
