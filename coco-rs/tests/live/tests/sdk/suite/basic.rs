//! Basic text-generation tests via `coco_inference::ModelRuntimeClient::query`.

use anyhow::Result;
use coco_inference::QueryParams;
use coco_llm_types::LlmMessage;

use crate::common::LiveTarget;
use crate::common::extract_text;
use crate::common::query_client;
use crate::common::usage_report;

const SYSTEM: &str = "You are a helpful assistant. Be concise.";

fn params_for(prompt: Vec<LlmMessage>, source: &str) -> QueryParams {
    QueryParams {
        prompt,
        // 1024 leaves headroom for reasoning models (gpt-5 etc.) that
        // burn the budget on hidden reasoning before emitting output.
        // Non-reasoning models still answer briefly — max_tokens is a
        // cap, not a target.
        max_tokens: Some(1024),
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: Some(source.to_string()),
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

/// Smallest possible round-trip — model returns the literal `hello`.
pub async fn run(target: &LiveTarget) -> Result<()> {
    let result = query_client(
        &target.client,
        params_for(
            vec![
                LlmMessage::system(SYSTEM),
                LlmMessage::user_text("Say 'hello' in exactly one word, nothing else."),
            ],
            "coco-tests-live::sdk::basic::run",
        ),
    )
    .await?;
    usage_report::record(target.provider, &target.model, "basic.run", &result.usage);

    let text = extract_text(&result);
    assert!(
        text.to_lowercase().contains("hello"),
        "{}/{}: expected 'hello' in response, got: {text}",
        target.provider,
        target.model
    );
    Ok(())
}

/// Provider returned a non-zero token usage payload — catches providers
/// whose `usage` block is absent or all-zero (would silently break cost
/// tracking and the cache-break detector).
pub async fn run_token_usage(target: &LiveTarget) -> Result<()> {
    let result = query_client(
        &target.client,
        params_for(
            vec![
                LlmMessage::system(SYSTEM),
                LlmMessage::user_text("Say 'hello'."),
            ],
            "coco-tests-live::sdk::basic::run_token_usage",
        ),
    )
    .await?;
    usage_report::record(
        target.provider,
        &target.model,
        "basic.token_usage",
        &result.usage,
    );

    assert!(
        result.usage.input_tokens.total > 0,
        "{}/{}: expected non-zero input tokens, usage={:?}",
        target.provider,
        target.model,
        result.usage
    );
    assert!(
        result.usage.output_tokens.total > 0,
        "{}/{}: expected non-zero output tokens, usage={:?}",
        target.provider,
        target.model,
        result.usage
    );
    Ok(())
}

/// Short multi-turn conversation: previously stated name must survive
/// into a follow-up assistant turn.
pub async fn run_multi_turn(target: &LiveTarget) -> Result<()> {
    let result = query_client(
        &target.client,
        params_for(
            vec![
                LlmMessage::system(SYSTEM),
                LlmMessage::user_text("My name is TestUser. Please remember it."),
                LlmMessage::assistant_text("Hello TestUser! I'll remember your name."),
                LlmMessage::user_text("What is my name?"),
            ],
            "coco-tests-live::sdk::basic::run_multi_turn",
        ),
    )
    .await?;
    usage_report::record(
        target.provider,
        &target.model,
        "basic.multi_turn",
        &result.usage,
    );

    let text = extract_text(&result);
    assert!(
        text.to_lowercase().contains("testuser"),
        "{}/{}: expected 'testuser' in response, got: {text}",
        target.provider,
        target.model
    );
    Ok(())
}

/// Long multi-turn: 12 alternating user/assistant messages building up
/// six factual claims, then a recall question that requires
/// remembering one of the early ones. Catches context-window /
/// truncation regressions and exercises the inference layer's
/// per-call message normalization at scale.
pub async fn run_long_multi_turn(target: &LiveTarget) -> Result<()> {
    // 6 fact-pairs (12 messages) + final recall question = 13 messages.
    let prompt = vec![
        LlmMessage::system(
            "You are a helpful assistant. Reply concisely. Always remember earlier facts.",
        ),
        // Turn 1
        LlmMessage::user_text(
            "Remember fact 1: my favorite color is teal. Acknowledge with one word.",
        ),
        LlmMessage::assistant_text("Acknowledged."),
        // Turn 2
        LlmMessage::user_text(
            "Remember fact 2: my dog's name is Mochi. Acknowledge with one word.",
        ),
        LlmMessage::assistant_text("Acknowledged."),
        // Turn 3
        LlmMessage::user_text("Remember fact 3: I work in Berlin. Acknowledge with one word."),
        LlmMessage::assistant_text("Acknowledged."),
        // Turn 4
        LlmMessage::user_text(
            "Remember fact 4: my coffee order is an oat-milk flat white. Acknowledge with one word.",
        ),
        LlmMessage::assistant_text("Acknowledged."),
        // Turn 5
        LlmMessage::user_text(
            "Remember fact 5: my preferred IDE is Helix. Acknowledge with one word.",
        ),
        LlmMessage::assistant_text("Acknowledged."),
        // Turn 6
        LlmMessage::user_text(
            "Remember fact 6: my keyboard is a Moonlander. Acknowledge with one word.",
        ),
        LlmMessage::assistant_text("Acknowledged."),
        // Recall — must reach back to fact 1.
        LlmMessage::user_text(
            "Now answer: what is my favorite color (fact 1)? Respond with just the color name.",
        ),
    ];

    let result = query_client(
        &target.client,
        params_for(prompt, "coco-tests-live::sdk::basic::run_long_multi_turn"),
    )
    .await?;
    usage_report::record(
        target.provider,
        &target.model,
        "basic.long_multi_turn",
        &result.usage,
    );

    let text = extract_text(&result).to_lowercase();
    assert!(
        text.contains("teal"),
        "{}/{}: long multi-turn recall failed; expected 'teal', got: {text}",
        target.provider,
        target.model
    );
    Ok(())
}
