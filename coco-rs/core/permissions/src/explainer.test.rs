use coco_types::RiskLevel;
use coco_types::SideQueryResponse;
use coco_types::SideQueryStopReason;
use coco_types::SideQueryToolUse;
use coco_types::SideQueryUsage;
use serde_json::json;

use super::*;

fn make_tool_response(tool_input: serde_json::Value) -> SideQueryResponse {
    SideQueryResponse {
        text: None,
        tool_uses: vec![SideQueryToolUse {
            name: "explain_command".to_string(),
            input: tool_input,
            invalid: false,
        }],
        stop_reason: SideQueryStopReason::ToolUse,
        usage: SideQueryUsage::default(),
        model_used: "test-model".to_string(),
    }
}

// ── build_explainer_query ──

#[test]
fn test_build_query_basic() {
    let params = ExplainerParams {
        tool_name: "Bash",
        tool_input: &json!({"command": "rm -rf /tmp/old"}),
        tool_description: None,
        messages: None,
    };
    let query = build_explainer_query(&params);

    assert!(query.messages[0].content.contains("Tool: Bash"));
    assert!(query.messages[0].content.contains("rm -rf /tmp/old"));
    assert_eq!(query.forced_tool, Some("explain_command".to_string()));
    assert_eq!(query.system, SYSTEM_PROMPT);
}

#[test]
fn test_build_query_with_description() {
    let params = ExplainerParams {
        tool_name: "Bash",
        tool_input: &json!({"command": "ls"}),
        tool_description: Some("Execute a shell command"),
        messages: None,
    };
    let query = build_explainer_query(&params);

    assert!(
        query.messages[0]
            .content
            .contains("Description: Execute a shell command")
    );
}

// ── tool schema ──

#[test]
fn test_tool_schema_fields() {
    let schema = explainer_tool_def();
    assert_eq!(schema.name, "explain_command");

    let props = schema.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("explanation"));
    assert!(props.contains_key("reasoning"));
    assert!(props.contains_key("risk"));
    assert!(props.contains_key("riskLevel"));
}

// ── response parsing ──

#[test]
fn test_parse_valid_response() {
    let json = json!({
        "riskLevel": "LOW",
        "explanation": "Lists directory contents",
        "reasoning": "I need to see what files exist",
        "risk": "None"
    });
    let result = parse_explainer_response(&json, "Bash");
    assert!(result.is_some());

    let explanation = result.unwrap();
    assert_eq!(explanation.risk_level, RiskLevel::Low);
    assert_eq!(explanation.explanation, "Lists directory contents");
    assert_eq!(explanation.reasoning, "I need to see what files exist");
}

#[test]
fn test_parse_high_risk() {
    let json = json!({
        "riskLevel": "HIGH",
        "explanation": "Deletes all files recursively",
        "reasoning": "I was asked to clean up",
        "risk": "Irreversible data loss"
    });
    let result = parse_explainer_response(&json, "Bash");
    assert!(result.is_some());
    assert_eq!(result.unwrap().risk_level, RiskLevel::High);
}

#[test]
fn test_parse_invalid_risk_level() {
    let json = json!({
        "riskLevel": "EXTREME",
        "explanation": "test",
        "reasoning": "test",
        "risk": "test"
    });
    assert!(parse_explainer_response(&json, "Bash").is_none());
}

#[test]
fn test_parse_missing_field() {
    let json = json!({
        "riskLevel": "LOW",
        "explanation": "test"
        // missing reasoning and risk
    });
    assert!(parse_explainer_response(&json, "Bash").is_none());
}

// ── async integration ──

#[tokio::test]
async fn test_generate_explanation_success() {
    let params = ExplainerParams {
        tool_name: "Bash",
        tool_input: &json!({"command": "git status"}),
        tool_description: None,
        messages: None,
    };

    let result = generate_permission_explanation(params, |_query| async {
        Ok(make_tool_response(json!({
            "riskLevel": "LOW",
            "explanation": "Shows git working tree status",
            "reasoning": "I need to check for uncommitted changes",
            "risk": "Read-only, no risk"
        })))
    })
    .await;

    assert!(result.is_some());
    let expl = result.unwrap();
    assert_eq!(expl.risk_level, RiskLevel::Low);
    assert!(expl.explanation.contains("git"));
}

#[tokio::test]
async fn test_generate_explanation_error_returns_none() {
    let params = ExplainerParams {
        tool_name: "Bash",
        tool_input: &json!({"command": "ls"}),
        tool_description: None,
        messages: None,
    };

    let result =
        generate_permission_explanation(params, |_query| async { Err("API timeout".to_string()) })
            .await;

    assert!(result.is_none());
}

#[tokio::test]
async fn test_generate_explanation_no_tool_use_returns_none() {
    let params = ExplainerParams {
        tool_name: "Bash",
        tool_input: &json!({"command": "ls"}),
        tool_description: None,
        messages: None,
    };

    let result = generate_permission_explanation(params, |_query| async {
        Ok(SideQueryResponse {
            text: Some("no structured output".to_string()),
            tool_uses: vec![],
            stop_reason: SideQueryStopReason::EndTurn,
            usage: SideQueryUsage::default(),
            model_used: "test".to_string(),
        })
    })
    .await;

    assert!(result.is_none());
}
