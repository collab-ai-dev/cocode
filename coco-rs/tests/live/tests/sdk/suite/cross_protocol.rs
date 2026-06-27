//! Cross-protocol parity for the *same* DeepSeek model.
//!
//! `deepseek-v4-flash` is exposed at two endpoints — one OpenAI-compat
//! (`/v1`), one Anthropic-shaped (`/anthropic/v1`). Two scenarios:
//!
//! - [`run`] — sends the same one-shot factual prompt through each
//!   client independently. Catches "one of the protocols is broken"
//!   regressions.
//! - [`run_session_switch`] — builds a single accumulated message
//!   history that *alternates* protocols turn-by-turn. Validates that
//!   coco-rs's seam-aliased `LlmMessage` shape serializes /
//!   deserializes identically across both wire formats. This is what
//!   "switching DeepSeek API forms mid-conversation" looks like at the
//!   inference layer.

use anyhow::Result;
use coco_inference::QueryParams;
use coco_llm_types::LlmMessage;

use crate::common::LiveTarget;
use crate::common::extract_text;
use crate::common::query_client;
use crate::common::usage_report;

fn factual_params(prompt: Vec<LlmMessage>, scenario: &str) -> QueryParams {
    QueryParams {
        prompt,
        max_tokens: Some(96),
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: Some(format!("coco-tests-live::sdk::cross_protocol::{scenario}")),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    }
}

/// Side-by-side: same one-shot prompt, both clients respond.
pub async fn run(openai_target: &LiveTarget, anthropic_target: &LiveTarget) -> Result<()> {
    let prompt = || {
        vec![
            LlmMessage::system("You are a concise assistant."),
            LlmMessage::user_text(
                "What is the capital of France? Respond with just the city name.",
            ),
        ]
    };

    let openai_result = query_client(
        &openai_target.client,
        factual_params(prompt(), "run.openai"),
    )
    .await?;
    usage_report::record(
        openai_target.provider,
        &openai_target.model,
        "cross_protocol.run",
        &openai_result.usage,
    );

    let anthropic_result = query_client(
        &anthropic_target.client,
        factual_params(prompt(), "run.anthropic"),
    )
    .await?;
    usage_report::record(
        anthropic_target.provider,
        &anthropic_target.model,
        "cross_protocol.run",
        &anthropic_result.usage,
    );

    for (label, result) in [
        (openai_target.provider, &openai_result),
        (anthropic_target.provider, &anthropic_result),
    ] {
        let text = extract_text(result);
        assert!(
            text.to_lowercase().contains("paris"),
            "{label}: expected 'paris' in response, got: {text}"
        );
    }
    Ok(())
}

/// Continue one logical conversation across protocol boundaries.
///
/// Turn 1 (`openai`): facts about Paris.
/// Turn 2 (`anthropic`): continues with that history, asks about
///   Lisbon — must remember Paris was the prior topic.
/// Turn 3 (`openai`): continues again, asks about both — must
///   remember the Lisbon reply that came back via the *Anthropic*
///   protocol.
///
/// Three protocol switches, each carrying the accumulated assistant
/// content from the *other* protocol. Validates the seam-aliased
/// `LlmMessage` shape is wire-stable across both APIs.
pub async fn run_session_switch(
    openai_target: &LiveTarget,
    anthropic_target: &LiveTarget,
) -> Result<()> {
    let system = "You are a helpful geography assistant. Be concise.";

    // Turn 1 — openai protocol
    let mut history = vec![
        LlmMessage::system(system),
        LlmMessage::user_text("What is the capital of France? Respond with just the city name."),
    ];
    let r1 = query_client(
        &openai_target.client,
        factual_params(history.clone(), "switch.t1.openai"),
    )
    .await?;
    usage_report::record(
        openai_target.provider,
        &openai_target.model,
        "cross_protocol.session_switch",
        &r1.usage,
    );
    let t1_text = extract_text(&r1);
    assert!(
        t1_text.to_lowercase().contains("paris"),
        "{}/{}: turn 1 should mention 'paris', got: {t1_text}",
        openai_target.provider,
        openai_target.model
    );

    // Turn 2 — anthropic protocol, picking up the openai assistant reply
    history.push(LlmMessage::assistant_text(t1_text.trim()));
    history.push(LlmMessage::user_text(
        "Now: what is the capital of Portugal? Respond with just the city name.",
    ));
    let r2 = query_client(
        &anthropic_target.client,
        factual_params(history.clone(), "switch.t2.anthropic"),
    )
    .await?;
    usage_report::record(
        anthropic_target.provider,
        &anthropic_target.model,
        "cross_protocol.session_switch",
        &r2.usage,
    );
    let t2_text = extract_text(&r2);
    assert!(
        t2_text.to_lowercase().contains("lisbon"),
        "{}/{}: turn 2 (anthropic) should mention 'lisbon', got: {t2_text}",
        anthropic_target.provider,
        anthropic_target.model
    );

    // Turn 3 — back to openai, must remember BOTH prior facts
    history.push(LlmMessage::assistant_text(t2_text.trim()));
    history.push(LlmMessage::user_text(
        "List the two capitals you just told me, separated by a comma. \
         Respond with just `<city1>, <city2>` and nothing else.",
    ));
    let r3 = query_client(
        &openai_target.client,
        factual_params(history, "switch.t3.openai"),
    )
    .await?;
    usage_report::record(
        openai_target.provider,
        &openai_target.model,
        "cross_protocol.session_switch",
        &r3.usage,
    );
    let t3_text = extract_text(&r3).to_lowercase();
    assert!(
        t3_text.contains("paris") && t3_text.contains("lisbon"),
        "{}/{}: turn 3 should mention both capitals (Paris, Lisbon), got: {t3_text}",
        openai_target.provider,
        openai_target.model
    );
    Ok(())
}
