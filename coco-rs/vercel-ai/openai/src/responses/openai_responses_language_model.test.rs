use super::*;
use std::sync::Arc;
use vercel_ai_provider::LanguageModelV4CallOptions;

fn make_config() -> Arc<OpenAIConfig> {
    Arc::new(OpenAIConfig {
        provider: "openai.responses".into(),
        base_url: "https://api.openai.com/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
        full_url: None,
        chatgpt_subscription: false,
        reasoning_store: Default::default(),
    })
}

#[test]
fn get_args_basic() {
    let model = OpenAIResponsesLanguageModel::new("gpt-4o", make_config());
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
        temperature: Some(0.7),
        ..Default::default()
    };

    let (body, warnings) = model.get_args(&options).expect("get_args");
    assert!(warnings.is_empty());
    assert_eq!(body["model"], "gpt-4o");
    assert!(
        body["temperature"]
            .as_f64()
            .is_some_and(|v| (v - 0.7).abs() < 0.01)
    );
    assert!(body["input"].is_array());
}

fn provider_options_with_layout(instructions: &str) -> vercel_ai_provider::ProviderOptions {
    let mut po = vercel_ai_provider::ProviderOptions::default();
    let mut inner = std::collections::HashMap::new();
    inner.insert(
        "instructions".to_string(),
        serde_json::Value::String(instructions.to_string()),
    );
    po.set("prompt_layout", inner);
    po
}

#[test]
fn layout_instructions_promote_to_top_level_and_drop_system_from_input() {
    let model = OpenAIResponsesLanguageModel::new("gpt-4o", make_config());
    let options = LanguageModelV4CallOptions {
        prompt: vec![
            vercel_ai_provider::LanguageModelV4Message::System {
                content: vec![vercel_ai_provider::UserContentPart::Text(
                    vercel_ai_provider::TextPart {
                        text: "you are coco".into(),
                        provider_metadata: None,
                    },
                )],
                provider_options: None,
            },
            vercel_ai_provider::LanguageModelV4Message::User {
                content: vec![vercel_ai_provider::UserContentPart::Text(
                    vercel_ai_provider::TextPart {
                        text: "Hi".into(),
                        provider_metadata: None,
                    },
                )],
                provider_options: None,
            },
        ],
        provider_options: Some(provider_options_with_layout("you are coco")),
        ..Default::default()
    };

    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(body["instructions"], "you are coco");
    let input = body["input"].as_array().expect("input");
    // The System message must NOT also appear in input[]; the only
    // remaining item is the User turn.
    let has_system = input.iter().any(|item| {
        item.get("role")
            .and_then(|r| r.as_str())
            .is_some_and(|r| r == "system" || r == "developer")
    });
    assert!(
        !has_system,
        "System should be stripped from input[] when layout supplies instructions"
    );
}

#[test]
fn layout_instructions_win_over_provider_options_with_warning() {
    let model = OpenAIResponsesLanguageModel::new("gpt-4o", make_config());
    let mut po = provider_options_with_layout("layout-supplied");
    let mut openai_inner = std::collections::HashMap::new();
    openai_inner.insert(
        "instructions".to_string(),
        serde_json::Value::String("openai-options-supplied".to_string()),
    );
    po.set("openai", openai_inner);
    let options = LanguageModelV4CallOptions {
        prompt: vec![vercel_ai_provider::LanguageModelV4Message::User {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "Hi".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        }],
        provider_options: Some(po),
        ..Default::default()
    };

    let (body, warnings) = model.get_args(&options).expect("get_args");
    assert_eq!(body["instructions"], "layout-supplied");
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            vercel_ai_provider::Warning::Other { message, .. } if message.contains("layout wins")
        )),
        "expected a Warning::Other documenting layout precedence"
    );
}

#[test]
fn get_args_reasoning_model() {
    let model = OpenAIResponsesLanguageModel::new("o3", make_config());
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

    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(body["model"], "o3");
    assert_eq!(body["max_output_tokens"], 100);
    // Temperature should be omitted for reasoning models
    assert!(body.get("temperature").is_none());
}

fn make_config_with_store(policy: ResponsesStorePolicy) -> Arc<OpenAIConfig> {
    Arc::new(OpenAIConfig {
        provider: "openai.responses".into(),
        base_url: "https://api.openai.com/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
        full_url: None,
        chatgpt_subscription: false,
        reasoning_store: policy,
    })
}

fn reasoning_user_options() -> LanguageModelV4CallOptions {
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
fn reasoning_store_server_default_omits_store_and_include() {
    // Default policy: plain API-key reasoning models keep server-side state —
    // `store` is omitted and `reasoning.encrypted_content` is NOT requested.
    let model = OpenAIResponsesLanguageModel::new(
        "o3",
        make_config_with_store(ResponsesStorePolicy::ServerDefault),
    );
    let (body, _) = model.get_args(&reasoning_user_options()).expect("get_args");
    assert!(
        body.get("store").is_none(),
        "store omitted under ServerDefault"
    );
    let has_include = body
        .get("include")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().any(|e| e == "reasoning.encrypted_content"));
    assert!(!has_include, "encrypted_content include not requested");
}

#[test]
fn reasoning_store_stateless_forces_store_false_and_include() {
    // Opt-in `stateless`: reasoning models go store=false and auto-include
    // `reasoning.encrypted_content` so the chain round-trips.
    let model = OpenAIResponsesLanguageModel::new(
        "o3",
        make_config_with_store(ResponsesStorePolicy::Stateless),
    );
    let (body, _) = model.get_args(&reasoning_user_options()).expect("get_args");
    assert_eq!(body["store"], false, "store forced false under Stateless");
    let has_include = body
        .get("include")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().any(|e| e == "reasoning.encrypted_content"));
    assert!(has_include, "encrypted_content include requested");
}

#[test]
fn reasoning_store_stateless_skipped_for_non_reasoning_model() {
    // The policy only affects reasoning models; gpt-4o stays store-free.
    let model = OpenAIResponsesLanguageModel::new(
        "gpt-4o",
        make_config_with_store(ResponsesStorePolicy::Stateless),
    );
    let (body, _) = model.get_args(&reasoning_user_options()).expect("get_args");
    assert!(
        body.get("store").is_none(),
        "non-reasoning model unaffected"
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
    let model = OpenAIResponsesLanguageModel::new("gpt-4o", make_config());
    let options = LanguageModelV4CallOptions {
        parallel_tool_calls: Some(true),
        ..simple_user_options()
    };
    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(
        body["parallel_tool_calls"], true,
        "Generic call-option toggle must emit snake_case `parallel_tool_calls` on the wire"
    );
}

#[test]
fn parallel_tool_calls_unset_omits_key() {
    let model = OpenAIResponsesLanguageModel::new("gpt-4o", make_config());
    let options = simple_user_options();
    let (body, _) = model.get_args(&options).expect("get_args");
    assert!(
        body.get("parallel_tool_calls").is_none(),
        "Unset toggle must NOT emit the key — provider default applies"
    );
}

#[test]
fn parallel_tool_calls_typed_provider_option_wins_over_generic() {
    let model = OpenAIResponsesLanguageModel::new("gpt-4o", make_config());
    let mut po = vercel_ai_provider::ProviderOptions::default();
    let mut inner = std::collections::HashMap::new();
    inner.insert("parallelToolCalls".into(), serde_json::Value::Bool(false));
    po.set("openai", inner);

    let options = LanguageModelV4CallOptions {
        provider_options: Some(po),
        parallel_tool_calls: Some(true),
        ..simple_user_options()
    };
    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(
        body["parallel_tool_calls"], false,
        "Typed provider_options.parallelToolCalls must win over generic flag"
    );
}
