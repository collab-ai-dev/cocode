use coco_config::MoaEndpointSpec;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_llm_types::ToolResultPart;
use coco_types::ModelSpec;
use coco_types::ProviderApi;

use super::*;

fn advisor(provider: &str, model_id: &str) -> coco_config::RoleSlot<ModelSpec> {
    coco_config::RoleSlot {
        model: spec(provider, model_id),
        effort: None,
    }
}

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
        reference_models: vec![advisor("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::PerIteration,
        reference_timeout_secs: None,
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
        reference_models: vec![advisor("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::UserTurn,
        reference_timeout_secs: None,
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
    let turn_1 = coco_types::TurnId::from("turn-1");
    let turn_2 = coco_types::TurnId::from("turn-2");

    let key = user_turn_cache_key(&endpoint, &base_prompt, &turn_1).expect("cache key");
    assert_eq!(
        key,
        user_turn_cache_key(&endpoint, &later_iteration_prompt, &turn_1).expect("cache key"),
        "user_turn fanout should reuse references within the same user turn",
    );
    assert_eq!(
        key,
        user_turn_cache_key(&endpoint, &no_system_prompt, &turn_1).expect("cache key"),
        "synthetic reference system prompt must not participate in the cache signature",
    );
    assert_ne!(
        key,
        user_turn_cache_key(&endpoint, &base_prompt, &turn_2).expect("cache key"),
        "reference outputs must not be reused across turns",
    );
}

#[test]
fn per_iteration_cache_key_is_disabled() {
    let endpoint = MoaEndpointSpec {
        preset_name: "default".to_string(),
        aggregator: spec("anthropic", "claude-sonnet-4-6"),
        reference_models: vec![advisor("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::PerIteration,
        reference_timeout_secs: None,
    };
    let prompt = vec![LlmMessage::user_text("prompt")];
    let turn_id = coco_types::TurnId::from("turn-1");

    assert!(user_turn_cache_key(&endpoint, &prompt, &turn_id).is_none());
}

#[tokio::test]
async fn moa_events_surface_reference_lifecycle_and_thinking_block() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let event_tx = Some(tx);
    let endpoint = MoaEndpointSpec {
        preset_name: "default".to_string(),
        aggregator: spec("anthropic", "claude-sonnet-4-6"),
        reference_models: vec![advisor("openai", "gpt-5-4")],
        fanout: coco_config::MoaFanout::PerIteration,
        reference_timeout_secs: None,
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

    let turn_id = coco_types::TurnId::from("turn-1");
    emit_reference_started(&event_tx, &turn_id, coco_types::ModelRole::Plan, &endpoint);
    emit_reference_completed(
        &event_tx,
        &turn_id,
        coco_types::ModelRole::Plan,
        &endpoint,
        &output,
    )
    .await;
    emit_moa_aggregating(&event_tx, &turn_id, coco_types::ModelRole::Plan, &endpoint).await;
    emit_reference_thinking_blocks(&event_tx, &turn_id, &[output]).await;

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

// ── N7c: per-advisor prompt fitting ──
//
// A preset mixes a small cheap advisor with a large one. One shared byte
// budget either overflows the small model (400 / silent provider truncation) or
// wastes the large one's room, so the prompt is fitted to each advisor's own
// declared window.

/// Build an advisor-shaped prompt: system + `turns` alternating user/assistant
/// pairs, ending on a user question.
fn advisor_prompt(turns: usize, body: &str) -> LlmPrompt {
    let mut out = vec![LlmMessage::system(REFERENCE_SYSTEM_PROMPT)];
    for i in 0..turns {
        out.push(LlmMessage::user_text(format!("u{i} {body}")));
        out.push(LlmMessage::assistant_text(format!("a{i} {body}")));
    }
    out.push(LlmMessage::user_text("the actual question"));
    out
}

#[test]
fn fit_prompt_passes_through_when_it_already_fits() {
    let prompt = advisor_prompt(2, "short");
    let before = prompt.len();
    let fitted = fit_prompt_to_advisor(prompt, Some(1_000_000), "openai", "gpt-5-4");
    assert_eq!(fitted.len(), before);
}

/// Unknown model (registry could not resolve it) must behave exactly as before
/// per-advisor fitting existed — never invent a bound.
#[test]
fn fit_prompt_passes_through_when_the_budget_is_unknown() {
    let prompt = advisor_prompt(40, "padding padding padding padding");
    let before = prompt.len();
    let fitted = fit_prompt_to_advisor(prompt, None, "openai", "gpt-5-4");
    assert_eq!(fitted.len(), before);
}

#[test]
fn fit_prompt_drops_oldest_turns_and_keeps_system_plus_question() {
    let prompt = advisor_prompt(40, &"padding ".repeat(40));
    let before = prompt.len();
    let fitted = fit_prompt_to_advisor(prompt, Some(2_000), "openai", "gpt-5-4");

    assert!(fitted.len() < before, "expected turns to be dropped");
    // The advisor instructions and the question are load-bearing: without the
    // first it has no role, without the last it has nothing to answer.
    assert!(matches!(fitted.first(), Some(LlmMessage::System { .. })));
    let LlmMessage::User { content, .. } = fitted.last().expect("tail") else {
        panic!("tail must stay a user message");
    };
    assert_eq!(user_text(content), "the actual question");
    let estimated: i64 = fitted.iter().map(estimate_advisor_message_tokens).sum();
    assert!(estimated <= 2_000, "still over budget: {estimated}");
}

/// Degenerate config: the question alone exceeds the advisor's window. Sending
/// as-is (a visible failure) beats silently clipping the question, which would
/// yield confidently wrong advice.
#[test]
fn fit_prompt_keeps_system_and_question_even_when_still_over_budget() {
    let prompt = advisor_prompt(3, "padding");
    let fitted = fit_prompt_to_advisor(prompt, Some(1), "openai", "gpt-5-4");
    assert_eq!(fitted.len(), 2);
    assert!(matches!(fitted.first(), Some(LlmMessage::System { .. })));
}
