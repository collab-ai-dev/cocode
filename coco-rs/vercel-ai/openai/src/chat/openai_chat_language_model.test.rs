use super::*;
use std::sync::Arc;
use vercel_ai_provider::LanguageModelV4CallOptions;

fn make_config() -> Arc<OpenAIConfig> {
    Arc::new(OpenAIConfig {
        provider: "openai.chat".into(),
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
    let model = OpenAIChatLanguageModel::new("gpt-4o", make_config());
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
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["max_tokens"], 100);
    assert!(body.get("max_completion_tokens").is_none());
}

#[test]
fn get_args_reasoning_model() {
    let model = OpenAIChatLanguageModel::new("o3", make_config());
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
    // Reasoning model should use max_completion_tokens, not max_tokens
    assert_eq!(body["max_completion_tokens"], 100);
    assert!(body.get("max_tokens").is_none());
    // Temperature should be omitted for reasoning models
    assert!(body.get("temperature").is_none());
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
    let model = OpenAIChatLanguageModel::new("gpt-4o", make_config());
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
fn parallel_tool_calls_generic_flag_emits_false() {
    let model = OpenAIChatLanguageModel::new("gpt-4o", make_config());
    let options = LanguageModelV4CallOptions {
        parallel_tool_calls: Some(false),
        ..simple_user_options()
    };
    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[test]
fn parallel_tool_calls_unset_omits_key() {
    let model = OpenAIChatLanguageModel::new("gpt-4o", make_config());
    let options = simple_user_options();
    let (body, _) = model.get_args(&options).expect("get_args");
    assert!(
        body.get("parallel_tool_calls").is_none(),
        "Unset toggle must NOT emit the key — provider default applies"
    );
}

#[test]
fn parallel_tool_calls_typed_provider_option_wins_over_generic() {
    // Typed provider_options.openai.parallelToolCalls = false must
    // override the generic call-options toggle (user-explicit beats
    // capability-driven default).
    let model = OpenAIChatLanguageModel::new("gpt-4o", make_config());
    let mut po = std::collections::HashMap::new();
    let mut inner = std::collections::HashMap::new();
    inner.insert("parallelToolCalls".into(), serde_json::Value::Bool(false));
    po.insert("openai".into(), inner);

    let options = LanguageModelV4CallOptions {
        provider_options: Some(vercel_ai_provider::ProviderOptions(po)),
        parallel_tool_calls: Some(true),
        ..simple_user_options()
    };
    let (body, _) = model.get_args(&options).expect("get_args");
    assert_eq!(
        body["parallel_tool_calls"], false,
        "Typed provider_options.parallelToolCalls must win over generic flag"
    );
}
