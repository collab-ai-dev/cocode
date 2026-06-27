//! HTTP-seam agent-loop e2e — drives a REAL `QueryEngine` whose model is
//! the genuine Anthropic provider pointed at a wiremock SSE server.
//!
//! Unlike the in-process `ScriptedModel` harnesses (which mock at the
//! `LanguageModel` trait and bypass the provider/codec/stream stack),
//! this exercises the full path:
//!   QueryEngine → ApiClient → AnthropicMessagesLanguageModel
//!     → HTTP POST /messages → wiremock SSE → codec → StreamAccumulator
//!     → tool execution → next request carrying the tool_result.
//!
//! It asserts the model-visible WIRE contract codex locks with
//! `validate_request_body_invariants`: a tool_use emitted on turn 1 is
//! paired with a tool_result on turn 2's request body, and NO request
//! ever carries an orphan tool_use/tool_result.
//!
//! Network-free + deterministic: the mock returns a tool_use turn until
//! it sees a tool_result in the request, then an end_turn turn. Fully
//! offline (no API key, no real Anthropic).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use coco_config::CatalogPaths;
use coco_config::RuntimeConfig;
use coco_config::RuntimeConfigBuilder;
use coco_config::RuntimeOverrides;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::ToolRegistry;
use coco_types::CoreEvent;
use coco_types::Features;
use coco_types::ModelSpec;
use coco_types::PermissionMode;
use coco_types::ProviderApi;
use coco_types::ProviderModelSelection;
use coco_types::ToolOverrides;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const TOOL_USE_ID: &str = "toolu_http_seam";
const MODEL_ID: &str = "claude-opus-4-7";

/// Build a fresh (non-cached) `RuntimeConfig` whose builtin `anthropic`
/// provider points at `base_url`. The cached `common::runtime::shared_runtime`
/// reads base_url from env once at process start, so it can't carry a
/// per-test wiremock URI — we synthesize a one-off `providers.json` overlay
/// instead. The returned `TempDir` backs `runtime.paths` and must outlive
/// the runtime.
fn runtime_with_anthropic_base_url(base_url: &str) -> Result<(Arc<RuntimeConfig>, TempDir)> {
    let home = tempfile::tempdir()?;
    let catalogs = CatalogPaths::empty_in(home.path());
    let overlay = json!({ "anthropic": { "base_url": base_url, "api_key": "test-key" } });
    if let Some(parent) = catalogs.providers.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&catalogs.providers, serde_json::to_string_pretty(&overlay)?)?;
    let overrides = RuntimeOverrides {
        model_override: Some(ProviderModelSelection {
            provider: "anthropic".into(),
            model_id: MODEL_ID.into(),
        }),
        ..Default::default()
    };
    let runtime = RuntimeConfigBuilder::from_process(home.path())
        .with_catalog_paths(catalogs)
        .with_overrides(overrides)
        .build()?;
    Ok((Arc::new(runtime), home))
}

/// Anthropic Messages SSE for a single `tool_use` turn (full input in one
/// `input_json_delta` chunk). Mirrors the shape locked by
/// `vercel-ai/anthropic/tests/messages_stream_tool_input_wiremock.rs`.
fn tool_use_sse(tool_use_id: &str, tool_name: &str, input_json: &str) -> String {
    sse_events(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","model":"claude-test","usage":{"input_tokens":10},"content":[]}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":tool_use_id,"name":tool_name,"input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":input_json}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ])
}

/// Anthropic Messages SSE for a plain end_turn text turn.
fn text_sse(text: &str) -> String {
    sse_events(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_2","model":"claude-test","usage":{"input_tokens":12},"content":[]}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ])
}

fn sse_events(events: &[(&str, serde_json::Value)]) -> String {
    let mut body = String::new();
    for (event, data) in events {
        body.push_str(&format!(
            "event: {event}\ndata: {}\n\n",
            serde_json::to_string(data).unwrap()
        ));
    }
    body
}

/// Assert the Anthropic request body has no orphan tool_use/tool_result:
/// every `tool_result.tool_use_id` must match an assistant `tool_use.id`.
/// This is coco's analogue of codex's `validate_request_body_invariants`.
fn assert_no_orphan_tool_pairing(body: &[u8]) {
    let v: serde_json::Value =
        serde_json::from_slice(body).expect("anthropic request body should be JSON");
    let mut tool_use_ids: HashSet<String> = HashSet::new();
    let messages = v["messages"].as_array();
    if let Some(msgs) = messages {
        for m in msgs {
            if m["role"] == "assistant"
                && let Some(content) = m["content"].as_array()
            {
                for p in content {
                    if p["type"] == "tool_use"
                        && let Some(id) = p["id"].as_str()
                    {
                        tool_use_ids.insert(id.to_string());
                    }
                }
            }
        }
        for m in msgs {
            if let Some(content) = m["content"].as_array() {
                for p in content {
                    if p["type"] == "tool_result" {
                        let tid = p["tool_use_id"]
                            .as_str()
                            .expect("tool_result must carry tool_use_id");
                        assert!(
                            tool_use_ids.contains(tid),
                            "orphan tool_result (tool_use_id={tid}) with no matching tool_use",
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn http_seam_tool_round_trip_pairs_tool_use_and_result() -> Result<()> {
    let workdir = tempfile::tempdir()?;

    let server = MockServer::start().await;
    // One responder for the whole turn loop: emit a Bash tool_use until the
    // request carries a tool_result, then close with an end_turn text turn.
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(|req: &wiremock::Request| {
            let body = String::from_utf8_lossy(&req.body);
            let sse = if body.contains("tool_result") {
                text_sse("Done — the shell command ran.")
            } else {
                tool_use_sse(TOOL_USE_ID, "Bash", r#"{"command":"echo http-seam-ok"}"#)
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream")
        })
        .mount(&server)
        .await;

    // Build the REAL Anthropic provider model through the approved
    // inference seam (`model_factory`), pointed at the mock via the
    // synthesized provider overlay. Direct `vercel-ai-anthropic` use is
    // forbidden outside `services/inference` by the seam guard.
    let (runtime, _home) = runtime_with_anthropic_base_url(&server.uri())?;
    let spec = ModelSpec {
        provider: "anthropic".to_string(),
        api: ProviderApi::Anthropic,
        model_id: MODEL_ID.to_string(),
        display_name: MODEL_ID.to_string(),
    };
    let model = coco_inference::model_factory::build_language_model_from_runtime(
        &runtime, &spec, /*resolver*/ None, /*header_vars*/ None,
    )
    .map_err(|e| anyhow::anyhow!("build anthropic model: {e}"))?;
    let model_runtimes = coco_query::test_support::model_runtime_registry(model);

    let tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(coco_tools::BashTool));
    let tools = Arc::new(tool_registry);

    let cfg = QueryEngineConfig {
        model_id: MODEL_ID.into(),
        permission_mode: PermissionMode::BypassPermissions,
        bypass_permissions_available: true,
        context_window: 200_000,
        max_output_tokens: 2_048,
        max_turns: Some(8),
        total_token_budget: None,
        system_prompt: Some("You are a test agent.".into()),
        is_non_interactive: true,
        project_dir: Some(workdir.path().to_path_buf()),
        cwd_override: Some(workdir.path().to_path_buf()),
        features: Arc::new(Features::with_defaults()),
        tool_overrides: Arc::new(ToolOverrides::none()),
        ..QueryEngineConfig::default()
    };

    let engine = QueryEngine::new(cfg, model_runtimes, tools, CancellationToken::new(), None);

    // Drain events so the engine never backpressures on a full channel.
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(512);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let result = engine
        .run_with_events("run the echo command", tx, coco_types::TurnId::generate())
        .await
        .map_err(|e| anyhow::anyhow!("engine run failed: {e}"))?;
    let _ = drain.await;

    assert!(!result.cancelled, "engine run should not be cancelled");

    // The mock saw the full round-trip over the wire.
    let requests = server
        .received_requests()
        .await
        .expect("wiremock should record requests");
    assert!(
        requests.len() >= 2,
        "expected ≥2 POST /messages (tool_use turn + tool_result feedback turn), got {}",
        requests.len(),
    );

    // The follow-up request carried the tool_result back through the REAL
    // codec — proving the provider/codec/stream path round-trips, not just
    // a trait mock.
    let second_body = String::from_utf8_lossy(&requests[1].body);
    assert!(
        second_body.contains("tool_result"),
        "2nd request should feed the tool_result back to the model; body: {second_body}",
    );
    assert!(
        second_body.contains(TOOL_USE_ID),
        "2nd request's tool_result should reference the tool_use id {TOOL_USE_ID}",
    );

    // Global invariant across EVERY captured request (codex parity).
    for req in &requests {
        assert_no_orphan_tool_pairing(&req.body);
    }

    Ok(())
}
