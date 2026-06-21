use super::*;
use coco_tool_runtime::SchemaIssue;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn normalize_value_string_recovers_object() {
    let mut input = json!("{\"path\": \"/tmp\"}");
    normalize_value_string(&mut input);
    assert_eq!(input, json!({"path": "/tmp"}));
}

#[test]
fn normalize_value_string_recovers_markdown_fence() {
    let mut input = json!("```json\n{\"path\": \"/tmp\"}\n```");
    normalize_value_string(&mut input);
    assert_eq!(input, json!({"path": "/tmp"}));
}

#[test]
fn normalize_value_string_keeps_non_object_recovery() {
    // String that parses to a number — schema validator should catch
    // the type mismatch; we keep the original String so the issue is
    // visible to the model.
    let mut input = json!("42");
    normalize_value_string(&mut input);
    assert_eq!(input, json!("42"));
}

#[test]
fn normalize_value_string_passes_through_object() {
    let mut input = json!({"path": "/tmp"});
    normalize_value_string(&mut input);
    assert_eq!(input, json!({"path": "/tmp"}));
}

#[test]
fn normalize_value_string_passes_through_other_types() {
    let mut input = json!(42);
    normalize_value_string(&mut input);
    assert_eq!(input, json!(42));

    let mut input = json!([1, 2, 3]);
    normalize_value_string(&mut input);
    assert_eq!(input, json!([1, 2, 3]));
}

#[test]
fn format_schema_error_single_missing_required() {
    let issues = vec![SchemaIssue::MissingRequired {
        path: String::new(),
        field: "command".to_string(),
    }];
    let out = format_schema_error("Bash", &issues);
    assert_eq!(
        out,
        "Bash failed due to the following issue:\nThe required parameter `command` is missing"
    );
}

#[test]
fn format_schema_error_multiple_issues_pluralizes() {
    let issues = vec![
        SchemaIssue::MissingRequired {
            path: String::new(),
            field: "command".to_string(),
        },
        SchemaIssue::TypeMismatch {
            path: "/timeout".to_string(),
            expected: "number".to_string(),
            received: "string".to_string(),
        },
    ];
    let out = format_schema_error("Bash", &issues);
    assert_eq!(
        out,
        "Bash failed due to the following issues:\n\
         The required parameter `command` is missing\n\
         The parameter `timeout` type is expected as `number` but provided as `string`"
    );
}

#[test]
fn format_schema_error_unexpected_field() {
    let issues = vec![SchemaIssue::UnexpectedField {
        path: String::new(),
        field: "extra_field".to_string(),
    }];
    let out = format_schema_error("Read", &issues);
    assert_eq!(
        out,
        "Read failed due to the following issue:\nAn unexpected parameter `extra_field` was provided"
    );
}

#[test]
fn format_schema_error_nested_path() {
    let issues = vec![SchemaIssue::TypeMismatch {
        path: "/edits/0/old_string".to_string(),
        expected: "string".to_string(),
        received: "number".to_string(),
    }];
    let out = format_schema_error("MultiEdit", &issues);
    assert_eq!(
        out,
        "MultiEdit failed due to the following issue:\n\
         The parameter `edits[0].old_string` type is expected as `string` but provided as `number`"
    );
}

#[test]
fn format_schema_error_empty_falls_back() {
    let out = format_schema_error("Tool", &[]);
    assert_eq!(out, "Tool failed schema validation");
}

// ---------------------------------------------------------------------------
// Multi-provider, multi-tool-call integration matrix.
//
// Simulates the assistant turn that arrives at schema validation: a batch of
// `ToolCallPart`s whose `input` already came out of wire parsing
// (`parse_tool_arguments_or_empty`). This is the exact shape every
// adapter — OpenAI Chat / OpenAI Responses / OpenAI-compat /
// Anthropic / Gemini — hands the agent loop, so the tests are
// provider-agnostic by design. Each entry pins:
//
//   (provider-style malformation) → (wire parsing outcome) → (schema validation
//   invalid_reason variant) → (formatted error message body)
// ---------------------------------------------------------------------------

use std::sync::Arc as StdArc;

use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::traits::DynTool;
use coco_types::ToolId;
use coco_types::ToolName;

use crate::tool_input_parse::parse_tool_arguments_or_empty;

/// Minimal Tool mock that lets each test pin its own input schema.
struct MockTool {
    id: ToolId,
    name: String,
    runtime_schema: coco_tool_runtime::ToolInputSchema,
}

impl MockTool {
    fn new(name: ToolName, schema: serde_json::Value) -> Self {
        Self {
            id: ToolId::Builtin(name),
            name: name.as_str().to_string(),
            runtime_schema: coco_tool_runtime::ToolInputSchema::from_value(schema)
                .expect("mock tool schema must be valid"),
        }
    }
}

#[async_trait::async_trait]
impl coco_tool_runtime::traits::Tool for MockTool {
    // Migration scaffold: assoc types pinned to `Value` — MockTool sets
    // schema dynamically via `input_json_schema` rather than driving it
    // from a typed `JsonSchema` struct.
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn id(&self) -> ToolId {
        self.id.clone()
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self, _input: &serde_json::Value, _options: &DescriptionOptions) -> String {
        String::new()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        "test tool".into()
    }
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        &self.runtime_schema
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolUseContext,
    ) -> Result<coco_messages::ToolResult<serde_json::Value>, ToolError> {
        unreachable!("MockTool::execute should not run in validate tests")
    }
}

fn bash_tool() -> StdArc<dyn DynTool> {
    StdArc::new(MockTool::new(
        ToolName::Bash,
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout": {"type": "number"},
            },
            "required": ["command"],
            "additionalProperties": false,
        }),
    ))
}

fn read_tool() -> StdArc<dyn DynTool> {
    StdArc::new(MockTool::new(
        ToolName::Read,
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "limit": {"type": "integer"},
            },
            "required": ["file_path"],
            "additionalProperties": false,
        }),
    ))
}

fn mk_tc(name: &str, input: serde_json::Value) -> ToolCallPart {
    ToolCallPart::new(format!("call_{name}"), name.to_string(), input)
}

/// Drive a raw `arguments` string through wire parsing + schema validation the same
/// way the adapter + tool_call_preparer do, then return the resulting
/// `ToolCallPart` so tests can assert against `invalid_reason`.
async fn run_pipeline(
    raw_arguments: &str,
    tool_name: &str,
    tool: Option<&StdArc<dyn DynTool>>,
) -> (ToolCallPart, Option<ValidatedInput>) {
    let input = parse_tool_arguments_or_empty(raw_arguments, tool_name);
    let mut tc = mk_tc(tool_name, input);
    let validated = validate_tool_call(&mut tc, tool);
    (tc, validated)
}

#[tokio::test]
async fn matrix_openai_clean_arguments_pass_validation() {
    let tool = read_tool();
    let (tc, _validated) = run_pipeline(r#"{"file_path": "/tmp/x"}"#, "Read", Some(&tool)).await;
    assert!(!tc.invalid);
    assert!(tc.invalid_reason.is_none());
    assert_eq!(tc.input, json!({"file_path": "/tmp/x"}));
}

#[tokio::test]
async fn matrix_openai_trailing_comma_is_repaired() {
    let tool = read_tool();
    let (tc, _validated) = run_pipeline(
        r#"{"file_path": "/tmp/x", "limit": 100,}"#,
        "Read",
        Some(&tool),
    )
    .await;
    assert!(
        !tc.invalid,
        "expected repair to succeed: {:?}",
        tc.invalid_reason
    );
    assert_eq!(tc.input, json!({"file_path": "/tmp/x", "limit": 100}));
}

#[tokio::test]
async fn matrix_glm_markdown_fence_is_repaired() {
    let tool = read_tool();
    let (tc, _validated) = run_pipeline(
        "```json\n{\"file_path\": \"/repo/main.rs\"}\n```",
        "Read",
        Some(&tool),
    )
    .await;
    assert!(!tc.invalid);
    assert_eq!(tc.input, json!({"file_path": "/repo/main.rs"}));
}

#[tokio::test]
async fn matrix_deepseek_single_quotes_is_repaired() {
    let tool = read_tool();
    let (tc, _validated) = run_pipeline(r#"{'file_path': '/tmp/foo'}"#, "Read", Some(&tool)).await;
    assert!(!tc.invalid);
    assert_eq!(tc.input, json!({"file_path": "/tmp/foo"}));
}

#[tokio::test]
async fn matrix_anthropic_streaming_truncated_recovers_value_but_misses_field() {
    // Recovered Value loses the required field — schema validation
    // surfaces the missing field rather than "JSON broken".
    let tool = bash_tool();
    let (tc, _validated) = run_pipeline(r#"{"unused_key": "/tmp"#, "Bash", Some(&tool)).await;
    assert!(tc.invalid);
    let reason = tc.invalid_reason.expect("invalid_reason set");
    let message = match reason {
        ToolInputInvalidReason::SchemaViolation { message } => message,
        other => panic!("expected SchemaViolation, got {other:?}"),
    };
    assert!(
        message.contains("required parameter `command` is missing"),
        "got: {message}"
    );
}

#[tokio::test]
async fn matrix_pure_garbage_falls_back_to_schema_missing_fields() {
    // wire parsing's `parse_with_repair` is intentionally aggressive —
    // `\u{0000}!!!@@@%%%` may resolve as `Value::Null` (the leading
    // NUL byte) or `Value::Object({})`. Either way the schema
    // validator must surface a usable error pointing at the bad
    // shape; the contract is "structured error, not opaque".
    let tool = bash_tool();
    let (tc, _validated) = run_pipeline("\u{0000}!!!@@@%%%", "Bash", Some(&tool)).await;
    assert!(tc.invalid);
    let message = match tc.invalid_reason.unwrap() {
        ToolInputInvalidReason::SchemaViolation { message } => message,
        other => panic!("expected SchemaViolation, got {other:?}"),
    };
    // Accept either the "missing required" or "type mismatch"
    // formatter line — both are valid schema validation outputs depending on
    // what wire parsing salvaged.
    let acceptable = message.contains("required parameter `command` is missing")
        || message.contains("expected as `object`");
    assert!(acceptable, "unexpected message: {message}");
}

#[tokio::test]
async fn matrix_empty_arguments_string_falls_back_to_schema_check() {
    let tool = bash_tool();
    let (tc, _validated) = run_pipeline("", "Bash", Some(&tool)).await;
    assert!(tc.invalid);
    let message = match tc.invalid_reason.unwrap() {
        ToolInputInvalidReason::SchemaViolation { message } => message,
        other => panic!("expected SchemaViolation, got {other:?}"),
    };
    assert!(message.contains("required parameter `command` is missing"));
}

#[tokio::test]
async fn matrix_type_mismatch_reports_expected_and_received() {
    let tool = bash_tool();
    let (tc, _validated) = run_pipeline(
        r#"{"command": "ls", "timeout": "not-a-number"}"#,
        "Bash",
        Some(&tool),
    )
    .await;
    assert!(tc.invalid);
    let message = match tc.invalid_reason.unwrap() {
        ToolInputInvalidReason::SchemaViolation { message } => message,
        other => panic!("expected SchemaViolation, got {other:?}"),
    };
    assert!(message.contains("`timeout`"), "got: {message}");
    assert!(message.contains("expected as `number`"), "got: {message}");
    assert!(message.contains("provided as `string`"), "got: {message}");
}

#[tokio::test]
async fn matrix_unexpected_field_is_reported() {
    let tool = bash_tool();
    let (tc, _validated) = run_pipeline(
        r#"{"command": "ls", "extra_field": "ignored"}"#,
        "Bash",
        Some(&tool),
    )
    .await;
    assert!(tc.invalid);
    let message = match tc.invalid_reason.unwrap() {
        ToolInputInvalidReason::SchemaViolation { message } => message,
        other => panic!("expected SchemaViolation, got {other:?}"),
    };
    assert!(
        message.contains("unexpected parameter `extra_field` was provided"),
        "got: {message}"
    );
}

#[tokio::test]
async fn matrix_no_such_tool_short_circuits_before_schema() {
    let (tc, _validated) = run_pipeline(r#"{"command": "ls"}"#, "NonexistentTool", None).await;
    assert!(tc.invalid);
    match tc.invalid_reason.unwrap() {
        ToolInputInvalidReason::NoSuchTool { tool_name } => {
            assert_eq!(tool_name, "NonexistentTool");
        }
        other => panic!("expected NoSuchTool, got {other:?}"),
    }
}

#[tokio::test]
async fn matrix_anthropic_value_string_nested_is_recovered_in_layer_2() {
    // Anthropic non-streaming path: the model nested stringified JSON
    // inside what should be the input object. wire parsing passes
    // `Value::String` through; schema validation's `normalize_value_string`
    // recovers it before schema validation.
    let tool = read_tool();
    let mut tc = mk_tc("Read", json!("{\"file_path\": \"/tmp/recovered\"}"));
    let validated = validate_tool_call(&mut tc, Some(&tool)).expect("recovered input validates");
    assert!(!tc.invalid);
    assert_eq!(
        validated.as_value(),
        &json!({"file_path": "/tmp/recovered"})
    );
    // The wire-shape `tc.input` is never mutated — the nested string
    // round-trips to the provider verbatim.
    assert_eq!(tc.input, json!("{\"file_path\": \"/tmp/recovered\"}"));
}

#[tokio::test]
async fn apply_patch_freeform_string_input_is_coerced_to_patch_object() {
    // The freeform `apply_patch` tool call arrives as a bare string (the raw
    // patch envelope). `validate_tool_call` must run the tool's
    // `coerce_raw_string_input` (→ `{patch: raw}`) before schema validation,
    // so the call is NOT marked invalid and the patch survives intact.
    let tool: StdArc<dyn DynTool> = StdArc::new(coco_tools::tools::ApplyPatchTool);
    let raw = "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch\n";
    let mut tc = mk_tc("apply_patch", json!(raw));
    let validated =
        validate_tool_call(&mut tc, Some(&tool)).expect("coerced patch must pass schema");
    assert!(!tc.invalid);
    assert_eq!(validated.as_value(), &json!({ "patch": raw }));
    // The wire-shape `tc.input` keeps the raw envelope for the provider
    // round-trip (OpenAI replays `custom_tool_call.input` as the string).
    assert_eq!(tc.input, json!(raw));

    // Regression: the second, serde-backed validator (`tool.validate_input`,
    // run by `tool_runner` / `tool_call_preparer`) must see the SAME coerced
    // input. Validating the raw `Value::String` envelope instead fails with
    // `invalid type: string` — the bug that surfaced once gpt-5 lost `Write`
    // and was forced onto `apply_patch`.
    let ctx = ToolUseContext::test_default();
    assert!(
        tool.validate_input(validated.as_value(), &ctx).is_valid(),
        "coerced patch object must pass the serde validator"
    );
    assert!(
        !tool.validate_input(&json!(raw), &ctx).is_valid(),
        "raw string must fail the serde validator — callers must coerce first"
    );
}

#[tokio::test]
async fn apply_patch_freeform_input_is_never_json_parsed() {
    // codex-rs parity: a freeform/custom tool's raw string is NEVER parsed as
    // JSON (codex routes it to `ToolPayload::Custom { input }`). Coercion must
    // run BEFORE `normalize_value_string`, so even a patch body that happens to
    // look like a JSON object is wrapped verbatim into `{patch: <raw>}` and not
    // mangled into the object itself.
    let tool: StdArc<dyn DynTool> = StdArc::new(coco_tools::tools::ApplyPatchTool);
    let json_looking = r#"{"patch": "not a real patch"}"#;
    let mut tc = mk_tc("apply_patch", json!(json_looking));
    let validated =
        validate_tool_call(&mut tc, Some(&tool)).expect("coerced patch must not be invalid");
    assert!(!tc.invalid);
    assert_eq!(
        validated.as_value(),
        &json!({ "patch": json_looking }),
        "raw freeform string must be wrapped verbatim, not JSON-parsed"
    );
}

#[tokio::test]
async fn matrix_layer_1_invalid_skips_layer_2() {
    // If wire parsing already set invalid (e.g., the adapter wanted to
    // signal a truly-unrecoverable case), schema validation must respect that
    // — don't overwrite the reason with its own classification.
    let tool = read_tool();
    let mut tc = mk_tc("Read", json!({"file_path": "/tmp/x"})).with_invalid_reason(
        ToolInputInvalidReason::JsonParseFailed {
            raw: "{garbage".to_string(),
            error: "expected `:` at line 1 column 2".to_string(),
        },
    );
    validate_tool_call(&mut tc, Some(&tool));
    assert!(tc.invalid);
    match tc.invalid_reason.unwrap() {
        ToolInputInvalidReason::JsonParseFailed { error, .. } => {
            // Reason preserved unchanged.
            assert!(error.contains("line 1"));
        }
        other => panic!("schema validation overwrote wire parsing's reason: {other:?}"),
    }
}

#[tokio::test]
async fn matrix_multi_tool_call_mixed_outcomes() {
    // Simulate a single assistant turn that emitted N tool calls
    // across providers — some clean, some malformed in different
    // ways. Each one should classify independently.
    let bash = bash_tool();
    let read = read_tool();

    struct Case {
        provider_hint: &'static str,
        tool_name: &'static str,
        raw_arguments: &'static str,
        tool: Option<StdArc<dyn DynTool>>,
        expected_valid: bool,
        expected_message_contains: Option<&'static str>,
    }

    let cases = vec![
        Case {
            provider_hint: "openai-chat clean",
            tool_name: "Read",
            raw_arguments: r#"{"file_path": "/tmp/a"}"#,
            tool: Some(read.clone()),
            expected_valid: true,
            expected_message_contains: None,
        },
        Case {
            provider_hint: "glm markdown fence",
            tool_name: "Read",
            raw_arguments: "```json\n{\"file_path\": \"/tmp/b\"}\n```",
            tool: Some(read.clone()),
            expected_valid: true,
            expected_message_contains: None,
        },
        Case {
            provider_hint: "deepseek single quotes",
            tool_name: "Bash",
            raw_arguments: r#"{'command': 'ls -la'}"#,
            tool: Some(bash.clone()),
            expected_valid: true,
            expected_message_contains: None,
        },
        Case {
            provider_hint: "openai trailing comma",
            tool_name: "Bash",
            raw_arguments: r#"{"command": "echo", "timeout": 5000,}"#,
            tool: Some(bash.clone()),
            expected_valid: true,
            expected_message_contains: None,
        },
        Case {
            provider_hint: "anthropic streaming truncated",
            tool_name: "Bash",
            raw_arguments: r#"{"command": "find ."#,
            tool: Some(bash.clone()),
            expected_valid: true,
            expected_message_contains: None,
        },
        Case {
            provider_hint: "missing required field",
            tool_name: "Bash",
            raw_arguments: r#"{"timeout": 1000}"#,
            tool: Some(bash.clone()),
            expected_valid: false,
            expected_message_contains: Some("required parameter `command` is missing"),
        },
        Case {
            provider_hint: "type mismatch",
            tool_name: "Read",
            raw_arguments: r#"{"file_path": "/tmp/x", "limit": "many"}"#,
            tool: Some(read.clone()),
            expected_valid: false,
            expected_message_contains: Some("expected as `integer`"),
        },
        Case {
            provider_hint: "hallucinated tool name",
            tool_name: "DoTheThing",
            raw_arguments: r#"{"x": 1}"#,
            tool: None,
            expected_valid: false,
            expected_message_contains: None,
        },
        Case {
            provider_hint: "empty arguments",
            tool_name: "Bash",
            raw_arguments: "",
            tool: Some(bash.clone()),
            expected_valid: false,
            expected_message_contains: Some("required parameter `command` is missing"),
        },
    ];

    for case in cases {
        let input = parse_tool_arguments_or_empty(case.raw_arguments, case.tool_name);
        let mut tc = mk_tc(case.tool_name, input);
        let _ = validate_tool_call(&mut tc, case.tool.as_ref());

        assert_eq!(
            !tc.invalid, case.expected_valid,
            "[{}] expected valid={}, got invalid_reason={:?}",
            case.provider_hint, case.expected_valid, tc.invalid_reason,
        );

        if let Some(needle) = case.expected_message_contains {
            let message = match tc.invalid_reason.as_ref() {
                Some(ToolInputInvalidReason::SchemaViolation { message }) => message.clone(),
                Some(ToolInputInvalidReason::NoSuchTool { tool_name }) => {
                    format!("NoSuchTool: {tool_name}")
                }
                Some(ToolInputInvalidReason::JsonParseFailed { error, .. }) => error.clone(),
                None => String::new(),
            };
            assert!(
                message.contains(needle),
                "[{}] expected message to contain `{}`, got: {}",
                case.provider_hint,
                needle,
                message
            );
        }
    }
}
