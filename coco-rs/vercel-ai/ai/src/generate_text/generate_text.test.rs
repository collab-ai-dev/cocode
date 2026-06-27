use std::sync::Arc;
use std::sync::Mutex;

use super::*;

use crate::test_utils::MockLanguageModel;
use crate::types::ProviderOptions;
use crate::types::SimpleTool;
use crate::types::ToolRegistry;

#[test]
fn test_generate_text_options() {
    let options = GenerateTextOptions::new("gpt-4", "Hello")
        .with_max_steps(5)
        .with_tool_choice(LanguageModelV4ToolChoice::auto());

    assert!(options.model.is_string());
    assert_eq!(options.max_steps, Some(5));
}

#[test]
fn test_generate_text_options_builders() {
    let options = GenerateTextOptions::new("gpt-4", "Hello")
        .with_max_steps(3)
        .with_active_tools(vec!["tool1".to_string()])
        .with_stop_when(super::super::stop_condition::step_count_is(5));

    assert_eq!(options.max_steps, Some(3));
    assert_eq!(options.active_tools, Some(vec!["tool1".to_string()]));
    assert_eq!(options.stop_when.len(), 1);
}

#[test]
fn test_prepare_step_overrides_default() {
    let overrides = PrepareStepOverrides::default();
    assert!(overrides.tool_choice.is_none());
    assert!(overrides.active_tools.is_none());
    assert!(overrides.model.is_none());
    assert!(overrides.system.is_none());
    assert!(overrides.provider_options.is_none());
}

#[tokio::test]
async fn test_generate_text_basic() {
    let model = MockLanguageModel::with_text("Hello, world!");
    let result = generate_text(GenerateTextOptions {
        model: crate::model::LanguageModel::from_v4(Arc::new(model)),
        prompt: crate::prompt::Prompt::user("Hi"),
        ..Default::default()
    })
    .await
    .unwrap();

    assert_eq!(result.text, "Hello, world!");
    assert!(!result.call_id.is_empty());
    assert_eq!(result.model_id, Some("mock-model".to_string()));
}

#[tokio::test]
async fn test_generate_text_with_callbacks() {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let started_clone = started.clone();
    let finished_clone = finished.clone();

    let model = MockLanguageModel::with_text("Response");
    let callbacks = GenerateTextCallbacks::new()
        .with_on_start(move |_| {
            started_clone.store(true, Ordering::SeqCst);
        })
        .with_on_finish(move |_| {
            finished_clone.store(true, Ordering::SeqCst);
        });

    let result = generate_text(GenerateTextOptions {
        model: crate::model::LanguageModel::from_v4(Arc::new(model)),
        prompt: crate::prompt::Prompt::user("Hi"),
        callbacks,
        ..Default::default()
    })
    .await
    .unwrap();

    assert_eq!(result.text, "Response");
    assert!(started.load(Ordering::SeqCst));
    assert!(finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_generate_text_error() {
    let model = MockLanguageModel::builder()
        .with_error("Test error")
        .build();

    let result = generate_text(GenerateTextOptions {
        model: crate::model::LanguageModel::from_v4(Arc::new(model)),
        prompt: crate::prompt::Prompt::user("Hi"),
        ..Default::default()
    })
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_generate_text_with_output_parsing() {
    let model = MockLanguageModel::builder()
        .with_text_response(r#"{"name": "test", "value": 42}"#)
        .build();

    let output = crate::generate_text::Output::new(serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "value": { "type": "integer" }
        }
    }));

    let result = generate_text(GenerateTextOptions {
        model: crate::model::LanguageModel::from_v4(Arc::new(model)),
        prompt: crate::prompt::Prompt::user("Generate JSON"),
        output: Some(output),
        ..Default::default()
    })
    .await
    .unwrap();

    assert!(result.output.is_some());
    let output_value = result.output.unwrap();
    assert_eq!(output_value["name"], "test");
    assert_eq!(output_value["value"], 42);
}

#[tokio::test]
async fn test_generate_text_passes_context_to_prepare_step_and_tool() {
    let prepared_runtime = Arc::new(Mutex::new(None::<String>));
    let prepared_runtime_clone = prepared_runtime.clone();
    let prepare_step: PrepareStepFn = Arc::new(move |ctx| {
        let runtime = ctx
            .runtime_context
            .as_ref()
            .and_then(|value| value.downcast_ref::<String>())
            .cloned();
        *prepared_runtime_clone
            .lock()
            .expect("lock prepared runtime") = runtime;
        None
    });

    let observed_tool_context = Arc::new(Mutex::new(None::<String>));
    let observed_tool_context_clone = observed_tool_context.clone();
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SimpleTool::with_name("lookup").handler(
        move |_input, options| {
            let observed_tool_context = observed_tool_context_clone.clone();
            async move {
                let context = options
                    .get_context::<String>()
                    .expect("tool context should be provided");
                *observed_tool_context.lock().expect("lock tool context") = Some(context.clone());
                Ok(serde_json::json!({ "context": context }))
            }
        },
    )));

    let model = MockLanguageModel::builder()
        .with_tool_call_response("call-1", "lookup", serde_json::json!({}))
        .build();

    let _ = generate_text(
        GenerateTextOptions::new(
            crate::model::LanguageModel::from_v4(Arc::new(model)),
            crate::prompt::Prompt::user("Hi"),
        )
        .with_tools(Arc::new(registry))
        .with_prepare_step(prepare_step)
        .with_runtime_context("runtime-context".to_string())
        .with_tool_context("lookup", "tool-context".to_string()),
    )
    .await
    .expect("generate_text should succeed");

    assert_eq!(
        *prepared_runtime.lock().expect("lock prepared runtime"),
        Some("runtime-context".to_string())
    );
    assert_eq!(
        *observed_tool_context.lock().expect("lock tool context"),
        Some("tool-context".to_string())
    );
}

#[tokio::test]
async fn test_generate_text_merges_prepare_step_provider_options() {
    let mut call_openai = std::collections::HashMap::new();
    call_openai.insert(
        "reasoning".to_string(),
        serde_json::json!({ "effort": "low", "summary": "auto" }),
    );
    call_openai.insert("store".to_string(), serde_json::json!(true));
    let mut call_options = ProviderOptions::new();
    call_options.set("openai", call_openai);

    let prepare_step: PrepareStepFn = Arc::new(|_ctx| {
        let mut step_openai = std::collections::HashMap::new();
        step_openai.insert(
            "reasoning".to_string(),
            serde_json::json!({ "effort": "high" }),
        );
        step_openai.insert("metadata".to_string(), serde_json::json!({ "a": 1 }));
        let mut step = ProviderOptions::new();
        step.set("openai", step_openai);
        Some(PrepareStepOverrides {
            provider_options: Some(step),
            ..Default::default()
        })
    });

    let model = Arc::new(MockLanguageModel::with_text("ok"));
    let model_for_assertions = model.clone();

    let _ = generate_text(
        GenerateTextOptions::new(
            crate::model::LanguageModel::from_v4(model),
            crate::prompt::Prompt::user("Hi"),
        )
        .with_provider_options(call_options)
        .with_prepare_step(prepare_step),
    )
    .await
    .expect("generate_text should succeed");

    let calls = model_for_assertions.generate_calls();
    assert_eq!(calls.len(), 1);
    let provider_options = calls[0]
        .provider_options
        .as_ref()
        .expect("provider options forwarded to model");
    let openai = provider_options
        .get("openai")
        .expect("openai provider options");
    assert_eq!(
        openai.get("reasoning"),
        Some(&serde_json::json!({ "effort": "high", "summary": "auto" })),
        "step reasoning.effort overrides call value; summary preserved from call"
    );
    assert_eq!(
        openai.get("store"),
        Some(&serde_json::json!(true)),
        "store from call options is preserved"
    );
    assert_eq!(
        openai.get("metadata"),
        Some(&serde_json::json!({ "a": 1 })),
        "metadata added by step is forwarded"
    );
}
