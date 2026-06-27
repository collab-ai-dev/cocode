use super::*;
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4StreamPart;

fn make_config() -> Arc<OpenAICompatibleConfig> {
    Arc::new(OpenAICompatibleConfig {
        provider: "xai.chat".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        query_params: None,
        client: None,
        include_usage: true,
        supports_structured_outputs: false,
        transform_request_body: None,
        metadata_extractor: None,
        supported_urls: None,
        error_handler: Arc::new(
            crate::openai_compatible_error::OpenAICompatibleFailedResponseHandler::new("xai"),
        ),
        prompt_tokens_total_semantics:
            crate::provider_options::PromptTokensTotalSemantics::Inclusive,
        provider_profile: crate::OpenAICompatibleProviderProfile::Generic,
        full_url: None,
    })
}

#[test]
fn get_args_basic() {
    let model = OpenAICompatibleChatLanguageModel::new("grok-2", make_config());
    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        temperature: Some(0.5),
        max_output_tokens: Some(100),
        ..Default::default()
    };

    let (body, warnings) = model.get_args(&options).expect("get_args");
    assert!(warnings.is_empty());
    assert_eq!(body["model"], "grok-2");
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["max_tokens"], 100);
}

#[test]
fn get_args_with_reasoning_effort() {
    let model = OpenAICompatibleChatLanguageModel::new("deepseek-r1", make_config());

    // Build proper ProviderOptions with nested HashMap
    let mut xai_opts = HashMap::new();
    xai_opts.insert(
        "reasoningEffort".into(),
        serde_json::Value::String("high".into()),
    );
    let mut provider_opts_map = HashMap::new();
    provider_opts_map.insert("xai".into(), xai_opts);

    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        provider_options: Some(vercel_ai_provider::ProviderOptions(provider_opts_map)),
        ..Default::default()
    };

    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn get_args_applies_transform_body() {
    let config = Arc::new(OpenAICompatibleConfig {
        provider: "custom.chat".into(),
        base_url: "https://api.example.com/v1".into(),
        headers: Arc::new(HashMap::new),
        query_params: None,
        client: None,
        include_usage: true,
        supports_structured_outputs: false,
        transform_request_body: Some(Arc::new(|mut body| {
            body["custom_field"] = Value::String("added".into());
            body
        })),
        metadata_extractor: None,
        supported_urls: None,
        error_handler: Arc::new(
            crate::openai_compatible_error::OpenAICompatibleFailedResponseHandler::new("xai"),
        ),
        prompt_tokens_total_semantics:
            crate::provider_options::PromptTokensTotalSemantics::Inclusive,
        provider_profile: crate::OpenAICompatibleProviderProfile::Generic,
        full_url: None,
    });

    let model = OpenAICompatibleChatLanguageModel::new("test-model", config);
    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        ..Default::default()
    };

    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(body["custom_field"], "added");
}

#[test]
fn provider_options_name_extracts_prefix() {
    let config = make_config();
    assert_eq!(config.provider_options_name(), "xai");
}

#[test]
fn config_url_with_query_params() {
    let config = OpenAICompatibleConfig {
        provider: "test.chat".into(),
        base_url: "https://api.example.com/v1".into(),
        headers: Arc::new(HashMap::new),
        query_params: Some(HashMap::from([("api-version".into(), "2024-01".into())])),
        client: None,
        include_usage: true,
        supports_structured_outputs: false,
        transform_request_body: None,
        metadata_extractor: None,
        supported_urls: None,
        error_handler: Arc::new(
            crate::openai_compatible_error::OpenAICompatibleFailedResponseHandler::new("test"),
        ),
        prompt_tokens_total_semantics:
            crate::provider_options::PromptTokensTotalSemantics::Inclusive,
        provider_profile: crate::OpenAICompatibleProviderProfile::Generic,
        full_url: None,
    };
    let url = config.url("/chat/completions");
    assert!(url.starts_with("https://api.example.com/v1/chat/completions?"));
    assert!(url.contains("api-version=2024-01"));
}

#[test]
fn get_args_warns_on_top_k() {
    let model = OpenAICompatibleChatLanguageModel::new("test", make_config());
    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        top_k: Some(10),
        ..Default::default()
    };

    let (_, warnings) = model.get_args(&options).expect("get_args");
    assert!(!warnings.is_empty());
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::Unsupported { feature, .. } if feature == "topK"))
    );
}

#[test]
fn get_args_passthrough_provider_specific_keys() {
    let model = OpenAICompatibleChatLanguageModel::new("grok-2", make_config());

    // Build provider options with both schema keys and passthrough keys
    let mut xai_opts = HashMap::new();
    xai_opts.insert("user".into(), Value::String("test-user".into()));
    xai_opts.insert("logitBias".into(), json!({"50256": -100.0}));
    xai_opts.insert("parallelToolCalls".into(), Value::Bool(true));
    xai_opts.insert("store".into(), Value::Bool(true));
    let mut provider_opts_map = HashMap::new();
    provider_opts_map.insert("xai".into(), xai_opts);

    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        provider_options: Some(vercel_ai_provider::ProviderOptions(provider_opts_map)),
        ..Default::default()
    };

    let (body, _) = model.get_args(&options).expect("get_args");
    // Schema key: extracted and set via typed field
    assert_eq!(body["user"], "test-user");
    // Passthrough keys: spread into body as-is
    assert_eq!(body["logitBias"], json!({"50256": -100.0}));
    assert_eq!(body["parallelToolCalls"], true);
    assert_eq!(body["store"], true);
}

#[test]
fn get_args_openai_compatible_fallback_key() {
    let model = OpenAICompatibleChatLanguageModel::new("test", make_config());

    // Use "openaiCompatible" key instead of provider name "xai"
    let mut compat_opts = HashMap::new();
    compat_opts.insert("reasoningEffort".into(), Value::String("low".into()));
    let mut provider_opts_map = HashMap::new();
    provider_opts_map.insert("openaiCompatible".into(), compat_opts);

    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        provider_options: Some(vercel_ai_provider::ProviderOptions(provider_opts_map)),
        ..Default::default()
    };

    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(body["reasoning_effort"], "low");
}

#[test]
fn get_args_warns_on_json_schema_fallback() {
    let model = OpenAICompatibleChatLanguageModel::new("test", make_config());
    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        response_format: Some(vercel_ai_provider::ResponseFormat::Json {
            schema: Some(json!({"type": "object", "properties": {"answer": {"type": "string"}}})),
            name: Some("test".into()),
            description: None,
        }),
        ..Default::default()
    };

    let (body, warnings) = model.get_args(&options).expect("get_args");
    // Should fall back to json_object since supports_structured_outputs is false
    assert_eq!(body["response_format"]["type"], "json_object");
    // Should emit a warning about the schema fallback
    assert!(warnings.iter().any(|w| {
        matches!(w, Warning::Unsupported { feature, .. } if feature == "responseFormat.schema")
    }));
}

async fn collect_chat_stream(chunks: Vec<Value>) -> Vec<LanguageModelV4StreamPart> {
    let data = chunks
        .into_iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>();
    let byte_stream: vercel_ai_provider_utils::ByteStream =
        Box::pin(futures::stream::iter(vec![Ok::<Bytes, reqwest::Error>(
            Bytes::from(data),
        )]));

    create_chat_stream(
        byte_stream,
        Vec::new(),
        false,
        "xai".into(),
        None,
        crate::provider_options::PromptTokensTotalSemantics::Inclusive,
    )
    .map(|part| part.expect("stream part"))
    .collect()
    .await
}

#[tokio::test]
async fn stream_buffers_tool_call_arguments_until_function_name_arrives() {
    let parts = collect_chat_stream(vec![
        json!({
            "id": "chatcmpl-1",
            "created": 1,
            "model": "test",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_late",
                        "type": "function",
                        "function": { "arguments": "" }
                    }]
                }
            }]
        }),
        json!({
            "id": "chatcmpl-1",
            "created": 1,
            "model": "test",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_late",
                        "type": "function",
                        "function": { "name": "test_tool", "arguments": "{\"" }
                    }]
                }
            }]
        }),
        json!({
            "id": "chatcmpl-1",
            "created": 1,
            "model": "test",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "value" }
                    }]
                }
            }]
        }),
        json!({
            "id": "chatcmpl-1",
            "created": 1,
            "model": "test",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "\":\"hi\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])
    .await;

    assert!(
        parts
            .iter()
            .all(|part| !matches!(part, LanguageModelV4StreamPart::Error { .. }))
    );

    let input_start = parts.iter().find_map(|part| match part {
        LanguageModelV4StreamPart::ToolInputStart { id, tool_name, .. } => {
            Some((id.as_str(), tool_name.as_str()))
        }
        _ => None,
    });
    assert_eq!(input_start, Some(("call_late", "test_tool")));

    let deltas = parts
        .iter()
        .filter_map(|part| match part {
            LanguageModelV4StreamPart::ToolInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas, vec!["{\"", "value", r#"":"hi"}"#]);

    let tool_call = parts.iter().find_map(|part| match part {
        LanguageModelV4StreamPart::ToolCall(call) => Some((
            call.tool_call_id.as_str(),
            call.tool_name.as_str(),
            call.input.as_str(),
        )),
        _ => None,
    });
    assert_eq!(
        tool_call,
        Some(("call_late", "test_tool", r#"{"value":"hi"}"#))
    );
}

#[tokio::test]
async fn stream_errors_when_pending_tool_call_never_receives_function_name() {
    let parts = collect_chat_stream(vec![
        json!({
            "id": "chatcmpl-1",
            "created": 1,
            "model": "test",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_missing_name",
                        "type": "function",
                        "function": { "arguments": "{\"value\":\"hi\"}" }
                    }]
                }
            }]
        }),
        json!({
            "id": "chatcmpl-1",
            "created": 1,
            "model": "test",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }]
        }),
    ])
    .await;

    let error = parts.iter().find_map(|part| match part {
        LanguageModelV4StreamPart::Error { error } => Some(error.message.as_str()),
        _ => None,
    });
    assert_eq!(
        error,
        Some("Invalid response data: Expected 'function.name' to be a string.")
    );
    assert!(
        parts
            .iter()
            .all(|part| !matches!(part, LanguageModelV4StreamPart::ToolCall(_)))
    );
}

// ─── parallel_tool_calls translation ─────────────────────────────

fn simple_user_options() -> LanguageModelV4CallOptions {
    LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hello".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        ..Default::default()
    }
}

#[test]
fn parallel_tool_calls_generic_flag_emits_snake_case_wire() {
    let model = OpenAICompatibleChatLanguageModel::new("grok-2", make_config());
    let options = LanguageModelV4CallOptions {
        parallel_tool_calls: Some(true),
        ..simple_user_options()
    };
    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(
        body["parallel_tool_calls"], true,
        "OpenAI-compatible providers expect OpenAI-standard snake_case wire key"
    );
}

#[test]
fn parallel_tool_calls_unset_omits_key() {
    let model = OpenAICompatibleChatLanguageModel::new("grok-2", make_config());
    let options = simple_user_options();
    let (body, _) = model.get_args(&options).expect("get_args");
    assert!(
        body.get("parallel_tool_calls").is_none(),
        "Unset toggle must NOT emit the key — provider default applies"
    );
}

#[test]
fn parallel_tool_calls_passthrough_provider_options_wins_over_generic() {
    // User-explicit override via `provider_options[<ns>].parallel_tool_calls`
    // (passthrough — snake_case) must win over the generic flag.
    // Passthrough runs AFTER the generic emission, so its value overwrites
    // anything the generic flag wrote earlier.
    let model = OpenAICompatibleChatLanguageModel::new("grok-2", make_config());
    // `provider_options_name()` strips the suffix after `.`, so
    // `xai.chat` resolves to namespace key `xai` for option lookup.
    let mut po = vercel_ai_provider::ProviderOptions::default();
    let mut inner = std::collections::HashMap::new();
    inner.insert("parallel_tool_calls".into(), serde_json::Value::Bool(false));
    po.set("xai", inner);

    let options = LanguageModelV4CallOptions {
        provider_options: Some(po),
        parallel_tool_calls: Some(true),
        ..simple_user_options()
    };
    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(
        body["parallel_tool_calls"], false,
        "User passthrough override must beat the capability-driven generic flag"
    );
}
