use coco_config::MoaEndpointSpec;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_llm_types::ToolResultPart;
use coco_types::ModelSpec;
use coco_types::ProviderApi;

use super::*;

fn spec(provider: &str, model_id: &str) -> ModelSpec {
    ModelSpec {
        provider: provider.to_string(),
        api: ProviderApi::OpenaiCompat,
        model_id: model_id.to_string(),
        display_name: model_id.to_string(),
    }
}

#[test]
fn reference_prompt_drops_system_and_textifies_tools() {
    let prompt = vec![
        LlmMessage::system("system secret"),
        LlmMessage::user_text("user question"),
        LlmMessage::assistant(vec![
            AssistantContentPart::text("assistant text"),
            AssistantContentPart::tool_call(
                "call_1",
                "Read",
                serde_json::json!({"file_path":"README.md"}),
            ),
        ]),
        LlmMessage::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                "call_1",
                "Read",
                ToolResultContent::text("tool output"),
            ))],
            provider_options: None,
        },
    ];

    let reference = reference_prompt(&prompt);
    assert!(
        !reference
            .iter()
            .any(|m| matches!(m, LlmMessage::Developer { .. } | LlmMessage::Tool { .. }))
    );
    let joined = serde_json::to_string(&reference).unwrap();
    assert!(!joined.contains("system secret"));
    assert!(joined.contains("reference advisor"));
    assert!(joined.contains("user question"));
    assert!(joined.contains("[called tool: Read"));
    assert!(joined.contains("tool output"));
    assert!(matches!(reference.last(), Some(LlmMessage::User { .. })));
}

#[test]
fn guidance_appends_to_api_prompt_clone_only() {
    let params = QueryParams {
        prompt: vec![LlmMessage::user_text("original")],
        ..Default::default()
    };
    let endpoint = MoaEndpointSpec {
        preset_name: "default".to_string(),
        aggregator: spec("anthropic", "claude-sonnet-4-6"),
        reference_models: vec![spec("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::PerIteration,
        reference_max_tokens: None,
        reference_temperature: None,
        aggregator_temperature: None,
    };
    let next = attach_reference_guidance(
        &params,
        &endpoint,
        &[ReferenceOutput {
            index: 0,
            count: 1,
            provider: "openai".to_string(),
            model_id: "gpt-5-4".to_string(),
            text: "reference advice".to_string(),
            failed: None,
            usage: None,
        }],
    );

    assert_eq!(params.prompt.len(), 1);
    assert_eq!(next.prompt.len(), 2);
    let encoded = serde_json::to_string(&next.prompt).unwrap();
    assert!(encoded.contains("reference advice"));
    assert!(encoded.contains("Mixture of Agents reference context"));
}

#[test]
fn user_turn_cache_key_is_turn_scoped_and_ignores_synthetic_context() {
    let endpoint = MoaEndpointSpec {
        preset_name: "default".to_string(),
        aggregator: spec("anthropic", "claude-sonnet-4-6"),
        reference_models: vec![spec("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::UserTurn,
        reference_max_tokens: None,
        reference_temperature: None,
        aggregator_temperature: None,
    };
    let base_prompt = vec![
        LlmMessage::system(REFERENCE_SYSTEM_PROMPT),
        LlmMessage::user_text("user question"),
    ];
    let later_iteration_prompt = vec![
        LlmMessage::system(REFERENCE_SYSTEM_PROMPT),
        LlmMessage::user_text("user question"),
        LlmMessage::assistant_text("tool loop context that should not alter user_turn key"),
        LlmMessage::user_text(ADVISORY_INSTRUCTION),
    ];
    let no_system_prompt = vec![LlmMessage::user_text("user question")];

    let key = user_turn_cache_key(&endpoint, &base_prompt, "turn-1").expect("cache key");
    assert_eq!(
        key,
        user_turn_cache_key(&endpoint, &later_iteration_prompt, "turn-1").expect("cache key"),
        "user_turn fanout should reuse references within the same user turn",
    );
    assert_eq!(
        key,
        user_turn_cache_key(&endpoint, &no_system_prompt, "turn-1").expect("cache key"),
        "synthetic reference system prompt must not participate in the cache signature",
    );
    assert_ne!(
        key,
        user_turn_cache_key(&endpoint, &base_prompt, "turn-2").expect("cache key"),
        "reference outputs must not be reused across turns",
    );
}

#[test]
fn per_iteration_cache_key_is_disabled() {
    let endpoint = MoaEndpointSpec {
        preset_name: "default".to_string(),
        aggregator: spec("anthropic", "claude-sonnet-4-6"),
        reference_models: vec![spec("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::PerIteration,
        reference_max_tokens: None,
        reference_temperature: None,
        aggregator_temperature: None,
    };
    let prompt = vec![LlmMessage::user_text("prompt")];

    assert!(user_turn_cache_key(&endpoint, &prompt, "turn-1").is_none());
}

#[tokio::test]
async fn moa_events_surface_reference_lifecycle_and_thinking_block() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let event_tx = Some(tx);
    let endpoint = MoaEndpointSpec {
        preset_name: "default".to_string(),
        aggregator: spec("anthropic", "claude-sonnet-4-6"),
        reference_models: vec![spec("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::PerIteration,
        reference_max_tokens: None,
        reference_temperature: None,
        aggregator_temperature: None,
    };
    let output = ReferenceOutput {
        index: 0,
        count: 1,
        provider: "openai".to_string(),
        model_id: "gpt-5-4".to_string(),
        text: "reference advice".to_string(),
        failed: None,
        usage: None,
    };

    emit_reference_started(&event_tx, "turn-1", coco_types::ModelRole::Plan, &endpoint);
    emit_reference_completed(
        &event_tx,
        "turn-1",
        coco_types::ModelRole::Plan,
        &endpoint,
        &output,
    )
    .await;
    emit_moa_aggregating(&event_tx, "turn-1", coco_types::ModelRole::Plan, &endpoint).await;
    emit_reference_thinking_blocks(&event_tx, "turn-1", &[output]).await;

    let started = rx.recv().await.expect("started");
    assert!(matches!(
        started,
        CoreEvent::Protocol(coco_types::ServerNotification::MoaReferenceStarted(_))
    ));
    let completed = rx.recv().await.expect("completed");
    let CoreEvent::Protocol(coco_types::ServerNotification::MoaReferenceCompleted(params)) =
        completed
    else {
        panic!("expected MoaReferenceCompleted");
    };
    assert_eq!(params.role, coco_types::ModelRole::Plan);
    assert_eq!(params.text, "reference advice");
    let aggregating = rx.recv().await.expect("aggregating");
    assert!(matches!(
        aggregating,
        CoreEvent::Protocol(coco_types::ServerNotification::MoaAggregating(_))
    ));
    let thinking = rx.recv().await.expect("thinking");
    let CoreEvent::Stream(coco_types::AgentStreamEvent::ThinkingDelta { delta, .. }) = thinking
    else {
        panic!("expected thinking delta");
    };
    assert!(delta.contains("MoA reference 1/1"));
    assert!(delta.contains("reference advice"));
}
