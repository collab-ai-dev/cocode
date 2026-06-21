//! End-to-end reasoning-chain round-trip for the OpenAI Responses API.
//!
//! This is the integration test that the original chain-loss regression
//! lacked: it drives a real OpenAI Responses SSE stream through the REAL
//! provider (`vercel-ai-openai`) AND the REAL inference snapshot accumulator
//! (`coco-inference::process_stream_with_config`), then feeds the rebuilt
//! assistant content back through `convert_to_openai_responses_input` — proving
//! `encrypted_content` survives every hop (capture → snapshot → sendback) and
//! that streamed tool calls correlate by `call_id`, not item id.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use tokio::sync::mpsc;
use vercel_ai_openai::OpenAIAuth;
use vercel_ai_openai::OpenAIProviderSettings;
use vercel_ai_openai::SystemMessageMode;
use vercel_ai_openai::create_openai;
use vercel_ai_openai::responses::convert_to_responses_input::convert_to_openai_responses_input;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::content::TextPart;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// A realistic OpenAI Responses SSE turn: a reasoning item (summary +
/// `encrypted_content`) followed by a streamed `function_call`.
const SSE: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"o3\"}}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\n\n",
    "data: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs_1\",\"delta\":\"thinking\"}\n\n",
    "data: {\"type\":\"response.reasoning_summary_text.done\",\"item_id\":\"rs_1\",\"text\":\"thinking\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"thinking\"}],\"encrypted_content\":\"ENC\"}}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_abc\",\"name\":\"Read\"}}\n\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"file_path\\\":\\\"/x\\\"}\"}\n\n",
    "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"arguments\":\"{\\\"file_path\\\":\\\"/x\\\"}\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_abc\",\"name\":\"Read\",\"arguments\":\"{\\\"file_path\\\":\\\"/x\\\"}\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
    "data: [DONE]\n\n",
);

fn openai_encrypted(meta: &Option<vercel_ai_provider::ProviderMetadata>) -> Option<String> {
    meta.as_ref()
        .and_then(|m| m.0.get("openai"))
        .and_then(|o| o.get("encryptedContent"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[tokio::test]
async fn reasoning_encrypted_content_round_trips_provider_to_inference_to_sendback() {
    // ── Wiremock OpenAI Responses endpoint ──────────────────────────────
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(SSE.to_string(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = create_openai(OpenAIProviderSettings {
        base_url: Some(server.uri()),
        auth: OpenAIAuth::ApiKey(Some("test-key".to_string())),
        ..Default::default()
    });
    let model = provider.responses("o3");
    let options = LanguageModelV4CallOptions {
        prompt: vec![LanguageModelV4Message::User {
            content: vec![UserContentPart::Text(TextPart::new("read /x"))],
            provider_options: None,
        }],
        ..Default::default()
    };

    // ── Provider stream → REAL inference snapshot accumulator ───────────
    let stream_result = model
        .do_stream(&options, None)
        .await
        .expect("do_stream opens");
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(coco_inference::stream::process_stream_with_config(
        stream_result.stream,
        tx,
        coco_inference::default_process_stream_config(),
    ));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let coco_inference::StreamEvent::Finish { snapshot: s, .. } = ev {
            snapshot = Some(s);
        }
    }
    let snapshot = snapshot.expect("a Finish event carries the snapshot");

    // Snapshot must preserve the encrypted reasoning blob and correlate the
    // tool call by `call_id`.
    let reasoning_seg = snapshot
        .parts
        .iter()
        .find_map(|p| match p {
            coco_inference::TurnPart::Reasoning(r) => Some(r),
            _ => None,
        })
        .expect("snapshot has a reasoning segment");
    assert_eq!(
        openai_encrypted(&reasoning_seg.provider_metadata).as_deref(),
        Some("ENC"),
        "inference snapshot preserved openai.encryptedContent"
    );
    let tool_seg = snapshot
        .parts
        .iter()
        .find_map(|p| match p {
            coco_inference::TurnPart::ToolCall(t) => Some(t),
            _ => None,
        })
        .expect("snapshot has a tool-call segment");
    assert_eq!(
        tool_seg.id, "call_abc",
        "tool call correlates by call_id, not item id"
    );

    // ── Rebuild assistant content (as the engine does) → sendback ───────
    let assistant = LanguageModelV4Message::Assistant {
        content: vec![
            AssistantContentPart::Reasoning(ReasoningPart {
                text: reasoning_seg.text.clone(),
                provider_metadata: reasoning_seg.provider_metadata.clone(),
            }),
            AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id: tool_seg.id.clone(),
                tool_name: tool_seg.tool_name.clone(),
                input: serde_json::json!({"file_path": "/x"}),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            }),
        ],
        provider_options: None,
    };
    let prompt: Vec<LanguageModelV4Message> = vec![assistant];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);

    let reasoning_item = items
        .iter()
        .find(|it| it["type"] == "reasoning")
        .expect("sendback emits a reasoning item");
    assert_eq!(
        reasoning_item["encrypted_content"], "ENC",
        "encrypted_content survives the full round-trip into the next request"
    );
    let fn_item = items
        .iter()
        .find(|it| it["type"] == "function_call")
        .expect("sendback emits the function_call");
    assert_eq!(
        fn_item["call_id"], "call_abc",
        "function_call_output will correlate by the wire call_id"
    );
}

/// The default config carries NO hard idle timeout (the original behavior —
/// idle abort is opt-in per provider via `stream_idle_timeout_secs`, applied by
/// `ApiClient::with_stream_idle_timeout`).
#[test]
fn default_stream_config_has_no_idle_timeout() {
    let cfg = coco_inference::default_process_stream_config();
    assert!(
        cfg.idle_timeout.is_none(),
        "idle timeout must default OFF (per-provider opt-in only)"
    );
}
