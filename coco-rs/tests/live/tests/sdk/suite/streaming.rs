//! Streaming tests via `coco_inference::ModelRuntimeClient::query_stream`.

use anyhow::Result;
use coco_inference::LanguageModelFunctionTool;
use coco_inference::LanguageModelTool;
use coco_inference::LanguageModelToolChoice;
use coco_inference::ModelCommunicationOutcome;
use coco_inference::QueryParams;
use coco_inference::StreamEvent;
use coco_llm_types::LlmMessage;
use coco_types::ThinkingLevel;

use crate::common::LiveTarget;
use crate::common::open_stream_client;
use crate::common::usage_report;
use crate::common::weather_tool_def;

/// Asserts: at least one event arrived, a `Finish` was emitted, and the
/// concatenated text contains `hello`.
pub async fn run(target: &LiveTarget) -> Result<()> {
    let params = QueryParams {
        prompt: vec![
            LlmMessage::system("You are a helpful assistant. Be concise."),
            LlmMessage::user_text("Say 'hello world' exactly."),
        ],
        // 16k removes max_tokens as a variable in any failure: the
        // model still stops naturally at ~150 tokens. Real flakes here
        // are gateway / model side (`stop_reason=Some(Error)` with 0
        // tokens = AIDP dropped the request; `Some(Other)` with no
        // text = Gemini finished mid-thought) and not a budget issue.
        max_tokens: Some(16_384),
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: Some("coco-tests-live::sdk::streaming::run".into()),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    };

    let (mut rx, token) = open_stream_client(&target.client, params).await?;
    let mut text = String::new();
    let mut events = 0usize;
    let mut saw_finish = false;
    let mut final_usage = coco_types::TokenUsage::default();

    while let Some(event) = rx.recv().await {
        events += 1;
        match event {
            StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
            StreamEvent::Finish { usage, .. } => {
                saw_finish = true;
                final_usage = usage;
            }
            _ => {}
        }
    }
    target.client.finish_call(
        &token,
        if saw_finish {
            ModelCommunicationOutcome::Success
        } else {
            ModelCommunicationOutcome::Failure
        },
    );
    usage_report::record(
        target.provider,
        &target.model,
        "streaming.run",
        &final_usage,
    );

    assert!(
        events > 0,
        "{}/{}: stream produced no events",
        target.provider,
        target.model
    );
    assert!(
        saw_finish,
        "{}/{}: stream did not emit Finish event",
        target.provider, target.model
    );
    assert!(
        text.to_lowercase().contains("hello"),
        "{}/{}: streamed text missing 'hello': {text}",
        target.provider,
        target.model
    );
    Ok(())
}

/// Streaming + tool-calling. Asserts a `ToolCallStart` event for `get_weather`.
///
/// Leaves `tool_choice` unset (the model decides) — relies on the imperative
/// prompt below to nudge tool use. Providers whose model spontaneously skips
/// the call (Gemini) should use [`run_with_tools_forced`] instead.
pub async fn run_with_tools(target: &LiveTarget) -> Result<()> {
    run_with_tools_choice(target, None).await
}

/// Same as [`run_with_tools`] but forces the call via `tool_choice: Required`.
///
/// For Gemini, `tool_choice: None` + a prompt nudge is flaky: the model
/// sometimes answers in prose and never emits a `ToolCallStart`. Forcing the
/// choice removes that model nondeterminism. NOTE: this is opt-in per provider
/// because some models reject a forced `tool_choice` — e.g. DeepSeek's thinking
/// model returns HTTP 400 "Thinking mode does not support this tool_choice".
pub async fn run_with_tools_forced(target: &LiveTarget) -> Result<()> {
    run_with_tools_choice(target, Some(LanguageModelToolChoice::required())).await
}

async fn run_with_tools_choice(
    target: &LiveTarget,
    tool_choice: Option<LanguageModelToolChoice>,
) -> Result<()> {
    let params = QueryParams {
        // Imperative system prompt nudges the model toward calling the tool
        // rather than answering in prose (the default for `tool_choice: None`
        // callers). 16k removes max_tokens as a variable in failure analysis:
        // the model normally reasons ~100 tokens and emits the tool call. Real
        // flakes are gateway / model side, surfaced via `stop_reason` in the
        // assertion message below.
        prompt: vec![
            LlmMessage::system(
                "You are a helpful assistant. For weather questions you MUST call \
                 the get_weather tool — do not answer with prose, do not refuse, \
                 do not return an empty message.",
            ),
            LlmMessage::user_text(
                "What's the weather in Tokyo? Call get_weather with city='Tokyo'.",
            ),
        ],
        max_tokens: Some(16_384),
        thinking_level: None,
        fast_mode: false,
        tools: Some(vec![weather_tool_def()]),
        tool_choice,
        context_management: None,
        query_source: Some("coco-tests-live::sdk::streaming::run_with_tools".into()),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    };

    let (mut rx, token) = open_stream_client(&target.client, params).await?;
    let mut tool_name = String::new();
    let mut saw_tool_call_start = false;
    let mut text = String::new();
    let mut final_usage = coco_types::TokenUsage::default();
    let mut stop_reason: Option<coco_llm_types::StopReason> = None;
    let mut raw_stop_reason: Option<String> = None;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ToolCallStart {
                tool_name: name, ..
            } => {
                saw_tool_call_start = true;
                tool_name = name;
            }
            StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
            StreamEvent::Finish {
                usage,
                stop_reason: fr,
                ..
            } => {
                final_usage = usage;
                raw_stop_reason = fr.raw.clone();
                stop_reason = Some(fr.unified);
            }
            _ => {}
        }
    }
    target.client.finish_call(
        &token,
        if stop_reason.is_some() {
            ModelCommunicationOutcome::Success
        } else {
            ModelCommunicationOutcome::Failure
        },
    );
    usage_report::record(
        target.provider,
        &target.model,
        "streaming.with_tools",
        &final_usage,
    );

    let text_preview: String = text.chars().take(80).collect();
    assert!(
        saw_tool_call_start,
        "{}/{}: stream did not emit a ToolCallStart event \
         (stop_reason={:?}, raw_stop_reason={:?}, \
         text={text_preview:?}, \
         tokens_in={:?}, tokens_out={:?})",
        target.provider,
        target.model,
        stop_reason,
        raw_stop_reason,
        final_usage.input_tokens,
        final_usage.output_tokens,
    );
    assert_eq!(
        tool_name, "get_weather",
        "{}/{}: unexpected tool name: {tool_name}",
        target.provider, target.model
    );
    Ok(())
}

/// Regression suite for two Gemini-3-strict bugs that prior sdk_google
/// runs all missed because every other case passed `thinking_level: None`
/// and a single-field tool schema:
///
/// 1. **`thinkingConfig` root-leak.** Setting `thinking_level: Some(_)`
///    routes a `{"thinkingConfig": ..}` blob through
///    `provider_options["google"]`. Before the
///    `#[serde(flatten)] extra` fix, that key was shallow-merged at the
///    body root in addition to the correct nested write under
///    `generationConfig.thinkingConfig`. Gemini's REST API rejects the
///    duplicate with 400 "Unknown name 'thinkingConfig'".
///
/// 2. **Schema converter `anyOf`/`oneOf` with siblings.** Schemars emits
///    `{anyOf: [{type:"integer"}, {type:"null"}], default, description,
///    format}` for `Option<i64>` and `{oneOf: [{const: v, type:"string"},
///    ...]}` for Rust string enums with per-variant docstrings.
///    Gemini-3 strict mode rejects both shapes
///    (`"anyOf must be the only field set"` / `"didn't specify the type
///    field"`). The converter now flattens single-element unions and
///    coalesces singleton-`const` oneOf into `{type, enum: [..]}`.
///
/// Builds a tool schema with both shapes inline so the regression covers
/// `convert_json_schema_to_openapi_schema` on real schemars output, and
/// sets `thinking_level: Some(medium)` so the provider-options seam is
/// exercised end-to-end.
pub async fn run_thinking_with_option_typed_tools(target: &LiveTarget) -> Result<()> {
    let tool_with_option_and_enum = LanguageModelTool::Function(LanguageModelFunctionTool {
        name: "search_logs".into(),
        description: Some("Search recent logs for a query string.".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Substring to search for."
                },
                // schemars-style Option<i64> — exercises the
                // type-array + siblings flatten path.
                "limit": {
                    "type": ["integer", "null"],
                    "description": "Max matches to return.",
                    "format": "int64",
                    "default": null,
                },
                // schemars-style enum with per-variant docstring —
                // exercises the oneOf-of-const coalesce path.
                "mode": {
                    "description": "Output mode.",
                    "oneOf": [
                        {"const": "lines",  "description": "raw matching lines",   "type": "string"},
                        {"const": "counts", "description": "match counts per file","type": "string"},
                        {"const": "files",  "description": "matching file paths",  "type": "string"},
                    ],
                    "default": null,
                    "type": ["string", "null"],
                },
            },
            "required": ["query"],
        }),
        input_examples: None,
        strict: None,
        provider_options: None,
    });

    let params = QueryParams {
        prompt: vec![
            LlmMessage::system(
                "You are a tool-using assistant. For any search request you MUST call \
                 the search_logs tool — never answer in prose.",
            ),
            LlmMessage::user_text(
                "Search logs for query='timeout', limit=5, mode='lines'. Call search_logs.",
            ),
        ],
        // Gemini-3 with thinking burns a budget on reasoning before
        // any output. With `thinking_level: medium` + per-variant enum
        // schemas in the tool the model can spend several thousand
        // tokens reasoning. A tight cap → `MAX_TOKENS` finish with
        // empty content (no tool call). Generous 16k so the budget
        // does not become the variable under test.
        max_tokens: Some(16_384),
        // The key flag — this is what was missing in every other live
        // test and let the typed-fields-leak bug ship.
        thinking_level: Some(ThinkingLevel::medium()),
        fast_mode: false,
        tools: Some(vec![tool_with_option_and_enum]),
        tool_choice: None,
        context_management: None,
        query_source: Some(
            "coco-tests-live::sdk::streaming::run_thinking_with_option_typed_tools".into(),
        ),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    };

    let (mut rx, token) = open_stream_client(&target.client, params).await?;
    let mut saw_tool_call_start = false;
    let mut tool_name = String::new();
    let mut text = String::new();
    let mut final_usage = coco_types::TokenUsage::default();
    let mut stop_reason: Option<coco_llm_types::StopReason> = None;
    let mut raw_stop_reason: Option<String> = None;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ToolCallStart {
                tool_name: name, ..
            } => {
                saw_tool_call_start = true;
                tool_name = name;
            }
            StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
            StreamEvent::Finish {
                usage,
                stop_reason: fr,
                ..
            } => {
                final_usage = usage;
                raw_stop_reason = fr.raw.clone();
                stop_reason = Some(fr.unified);
            }
            _ => {}
        }
    }
    target.client.finish_call(
        &token,
        if stop_reason.is_some() {
            ModelCommunicationOutcome::Success
        } else {
            ModelCommunicationOutcome::Failure
        },
    );
    usage_report::record(
        target.provider,
        &target.model,
        "streaming.thinking_with_option_typed_tools",
        &final_usage,
    );

    // Surface both unified `stop_reason` and `raw_stop_reason` so
    // every distinct flake mode is interpretable from one assertion:
    //   `Some(Error)`     + 0 tokens          → AIDP gateway dropped the request
    //   `Some(Other)`     + raw=<wire-str>    → Gemini unmapped finish (RECITATION etc)
    //   `Some(MaxTokens)` + tokens_out ≈ cap  → budget exhausted (real, bump max_tokens)
    //   any other         + text="..."        → model answered in prose instead
    //   any other         + content empty     → wire-shape regression
    let text_preview: String = text.chars().take(80).collect();
    assert!(
        saw_tool_call_start,
        "{}/{}: stream did not emit a ToolCallStart event \
         (stop_reason={:?}, raw_stop_reason={:?}, \
         text={text_preview:?}, \
         tokens_in={:?}, tokens_out={:?})",
        target.provider,
        target.model,
        stop_reason,
        raw_stop_reason,
        final_usage.input_tokens,
        final_usage.output_tokens,
    );
    assert_eq!(
        tool_name, "search_logs",
        "{}/{}: unexpected tool name: {tool_name}",
        target.provider, target.model
    );
    Ok(())
}
