//! Prompt-cache prefix-stability regression guard for the DeepSeek /
//! OpenAI-compatible path.
//!
//! DeepSeek uses AUTOMATIC prefix caching: the server reuses cached
//! computation for the longest byte-identical message prefix it shares
//! with the previous request — there are NO explicit `cache_control`
//! breakpoints to fall back on (unlike Anthropic). So a warm cache
//! depends entirely on coco never mutating an already-sent message: each
//! turn must be append-only at the tail.
//!
//! This drives a REAL `QueryEngine` whose model is the genuine
//! openai-compatible provider (built through the approved
//! `coco_inference::model_factory` seam) pointed at a wiremock
//! `/chat/completions` server, runs a multi-request tool loop, captures
//! every outbound request body, and asserts the invariant:
//!
//!   request[i-1].messages is a byte-identical PREFIX of request[i].messages
//!
//! i.e. every prior message is replayed unchanged and only new content is
//! appended. This is coco's analogue of Reasonix's `cachehit_e2e_test.go`
//! (`hitChars[i] == reqChars[i-1]`). It locks in the append-and-freeze
//! reminder/history behavior so a future change (ephemeral reminders, a
//! mid-history reposition, a normalize pass that rewrites a sent message)
//! can't silently tank the DeepSeek cache hit rate.
//!
//! Fully offline (no API key, no real DeepSeek) and deterministic.

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
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const PROVIDER: &str = "deepseek-openai";
const MODEL_ID: &str = "deepseek-v4-flash";

/// Build a fresh `RuntimeConfig` whose builtin `deepseek-openai` provider
/// points at `base_url` (the wiremock server). Mirrors
/// `http_seam_tool_loop`'s anthropic overlay but for the OpenAI-compatible
/// vendor. The returned `TempDir` backs `runtime.paths` and must outlive
/// the runtime.
fn runtime_with_deepseek_base_url(base_url: &str) -> Result<(Arc<RuntimeConfig>, TempDir)> {
    let home = tempfile::tempdir()?;
    let catalogs = CatalogPaths::empty_in(home.path());
    let overlay = json!({ PROVIDER: { "base_url": base_url, "api_key": "test-key" } });
    if let Some(parent) = catalogs.providers.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&catalogs.providers, serde_json::to_string_pretty(&overlay)?)?;
    let overrides = RuntimeOverrides {
        model_override: Some(ProviderModelSelection {
            provider: PROVIDER.into(),
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

/// OpenAI Chat Completions SSE for one streamed `tool_calls` turn (full
/// arguments in a single delta). Mirrors the shape locked by
/// `vercel-ai/openai-compatible/tests/chat_stream_tool_input_wiremock.rs`.
fn tool_call_chat_sse(call_id: &str, name: &str, arguments: &str) -> String {
    let open = json!({
        "id": "chatcmpl-prefix", "object": "chat.completion.chunk",
        "created": 1_700_000_000, "model": "compat-test",
        "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{
            "index": 0, "id": call_id, "type": "function",
            "function": {"name": name, "arguments": ""},
        }]}, "finish_reason": null}],
    });
    let args = json!({
        "id": "chatcmpl-prefix", "object": "chat.completion.chunk",
        "created": 1_700_000_000, "model": "compat-test",
        "choices": [{"index": 0, "delta": {"tool_calls": [{
            "index": 0, "function": {"arguments": arguments},
        }]}, "finish_reason": null}],
    });
    let finish = json!({
        "id": "chatcmpl-prefix", "object": "chat.completion.chunk",
        "created": 1_700_000_000, "model": "compat-test",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 20, "completion_tokens": 5, "total_tokens": 25},
    });
    sse_chunks(&[open, args, finish])
}

/// OpenAI Chat Completions SSE for a plain `stop` text turn.
fn text_chat_sse(text: &str) -> String {
    let open = json!({
        "id": "chatcmpl-prefix", "object": "chat.completion.chunk",
        "created": 1_700_000_000, "model": "compat-test",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}],
    });
    let body = json!({
        "id": "chatcmpl-prefix", "object": "chat.completion.chunk",
        "created": 1_700_000_000, "model": "compat-test",
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}],
    });
    let finish = json!({
        "id": "chatcmpl-prefix", "object": "chat.completion.chunk",
        "created": 1_700_000_000, "model": "compat-test",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 30, "completion_tokens": 7, "total_tokens": 37},
    });
    sse_chunks(&[open, body, finish])
}

fn sse_chunks(chunks: &[serde_json::Value]) -> String {
    let mut body = String::new();
    for c in chunks {
        body.push_str(&format!("data: {}\n\n", serde_json::to_string(c).unwrap()));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// Number of tool-result messages already in the request — `tool_call_id`
/// appears only on `{"role":"tool",...}` results (assistant tool_calls use
/// `id`), so this counts completed tool rounds.
fn tool_result_count(body: &str) -> usize {
    body.matches("tool_call_id").count()
}

#[tokio::test]
async fn deepseek_request_prefix_is_byte_stable_across_turns() -> Result<()> {
    let workdir = tempfile::tempdir()?;

    let server = MockServer::start().await;
    // Drive a 2-round tool loop → 3 requests: emit a Bash tool_call until
    // the request carries two tool_results, then finish with text. Each
    // round uses a distinct call id so tool_use/tool_result pairing holds.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|req: &wiremock::Request| {
            let body = String::from_utf8_lossy(&req.body);
            let rounds = tool_result_count(&body);
            let sse = if rounds >= 2 {
                text_chat_sse("Done — both echo commands ran.")
            } else {
                tool_call_chat_sse(
                    &format!("call_{rounds}"),
                    "Bash",
                    &format!(r#"{{"command":"echo prefix-stable-{rounds}"}}"#),
                )
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream")
        })
        .mount(&server)
        .await;

    let (runtime, _home) = runtime_with_deepseek_base_url(&server.uri())?;
    let spec = ModelSpec {
        provider: PROVIDER.to_string(),
        api: ProviderApi::OpenaiCompat,
        model_id: MODEL_ID.to_string(),
        display_name: MODEL_ID.to_string(),
    };
    let model = coco_inference::model_factory::build_language_model_from_runtime(
        &runtime, &spec, /*resolver*/ None, /*header_vars*/ None,
    )
    .map_err(|e| anyhow::anyhow!("build deepseek-openai model: {e}"))?;
    let model_runtimes = coco_query::test_support::model_runtime_registry(model);

    let tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(coco_tools::BashTool));
    let tools = Arc::new(tool_registry);

    let cfg = QueryEngineConfig {
        model_id: MODEL_ID.into(),
        permission_mode: PermissionMode::BypassPermissions,
        bypass_permissions_available: true,
        context_window: 1_000_000,
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

    let (tx, mut rx) = mpsc::channel::<CoreEvent>(512);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let result = engine
        .run_with_events(
            "run two echo commands then finish",
            tx,
            coco_types::TurnId::generate(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("engine run failed: {e}"))?;
    let _ = drain.await;
    assert!(!result.cancelled, "engine run should not be cancelled");

    let requests = server
        .received_requests()
        .await
        .expect("wiremock should record requests");
    assert!(
        requests.len() >= 3,
        "expected ≥3 POST /chat/completions (2 tool rounds + final text), got {}",
        requests.len(),
    );

    // Extract the `messages` array from each captured request body.
    let message_arrays: Vec<Vec<serde_json::Value>> = requests
        .iter()
        .map(|req| {
            let v: serde_json::Value = serde_json::from_slice(&req.body)
                .expect("openai-compat request body should be JSON");
            v["messages"]
                .as_array()
                .expect("request must carry a messages array")
                .clone()
        })
        .collect();

    // THE INVARIANT: each request's messages are a byte-identical prefix of
    // the next request's messages — only new turns are appended at the tail,
    // never a mutation of an already-sent message. This is exactly what keeps
    // DeepSeek's automatic prefix cache warm turn over turn.
    for i in 1..message_arrays.len() {
        let prev = &message_arrays[i - 1];
        let cur = &message_arrays[i];
        assert!(
            cur.len() > prev.len(),
            "request {i} should only GROW vs request {}: prev={} cur={}",
            i - 1,
            prev.len(),
            cur.len(),
        );
        for (j, prev_msg) in prev.iter().enumerate() {
            assert_eq!(
                &cur[j],
                prev_msg,
                "PREFIX BROKEN: request {i} message[{j}] differs from request {}'s — \
                 an already-sent message was mutated/reordered, which caps DeepSeek's \
                 automatically-cacheable prefix at this position",
                i - 1,
            );
        }
    }

    // The system prompt (messages[0]) — the most cache-critical block — must
    // be byte-stable across the entire session.
    let system0 = &message_arrays[0][0];
    for (i, msgs) in message_arrays.iter().enumerate() {
        assert_eq!(
            &msgs[0], system0,
            "request {i} mutated the system message — the cache-stable prefix head must never change mid-session",
        );
    }

    Ok(())
}
