use coco_messages::AssistantContent;
use coco_messages::MessageHistory;

use super::*;

fn assistant(text: &str) -> coco_messages::Message {
    coco_messages::create_assistant_message(
        vec![AssistantContent::text(text)],
        "test-model",
        coco_types::TokenUsage::default(),
    )
}

#[test]
fn compaction_drops_segments_whose_anchor_is_gone_and_keeps_the_rest_ordered() {
    let mut history = MessageHistory::new();
    let mut recovery = RecoveryWorkingContext::default();

    history.push(coco_messages::create_user_message("first durable"));
    recovery.append(
        &history,
        assistant("first malformed"),
        coco_messages::create_meta_message("first nudge"),
    );

    history.push(coco_messages::create_user_message("second durable"));
    recovery.append(
        &history,
        assistant("second malformed"),
        coco_messages::create_meta_message("second nudge"),
    );

    // Compaction replaced the earlier anchor. The first pair answered context
    // that no longer exists — relocating it would put a stale "you returned an
    // empty response" at the tail, pointing at nothing. The retained pair keeps
    // its position.
    let durable = vec![history.last().expect("second durable message").clone()];
    let assembled = recovery.assemble(&durable);
    let text: Vec<_> = assembled
        .iter()
        .map(|message| coco_messages::wrapping::extract_text_from_message(message))
        .collect();

    assert_eq!(text, ["second durable", "second malformed", "second nudge"]);
}

#[test]
fn request_context_precedes_recovery_but_durable_response_does_not() {
    let mut history = MessageHistory::new();
    let mut recovery = RecoveryWorkingContext::default();

    history.push(coco_messages::create_user_message("original request"));
    recovery.append(
        &history,
        assistant("malformed response"),
        coco_messages::create_meta_message("recovery nudge"),
    );

    let mut durable = history.to_vec();
    durable.push(std::sync::Arc::new(coco_messages::create_meta_message(
        "next request context",
    )));
    durable.push(std::sync::Arc::new(assistant("valid response")));

    let assembled = recovery.assemble(&durable);
    let text: Vec<_> = assembled
        .iter()
        .map(|message| coco_messages::wrapping::extract_text_from_message(message))
        .collect();

    assert_eq!(
        text,
        [
            "original request",
            "next request context",
            "malformed response",
            "recovery nudge",
            "valid response",
        ]
    );
}

#[test]
fn failure_stats_capture_every_cross_turn_accumulator() {
    let usage = TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 12,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: 7,
            ..Default::default()
        },
    };
    let denial = coco_types::PermissionDenialInfo {
        tool_name: "Bash".to_string(),
        tool_use_id: "toolu_denied".to_string(),
        tool_input: serde_json::json!({"command": "unsafe"}),
    };
    let mut acc = LoopAccumulator {
        api_time_ms: 37,
        total_usage: usage,
        permission_denials: vec![denial],
        ..Default::default()
    };
    acc.cost_tracker
        .record_usage("anthropic", "claude-sonnet-4-6", usage, 37);
    let mut turn_state = LoopTurnState::new(None, None, 3);
    turn_state.turn = 4;
    let consts = LoopConstants {
        started_at: std::time::Instant::now() - std::time::Duration::from_millis(50),
        user_uuid: "user".to_string(),
        plans_dir: None,
        todo_key: "session".to_string(),
        context_window: 100_000,
        effective_window: 90_000,
    };

    let mut stats = QueryFailureStats::default();
    stats.capture(&acc, &turn_state, &consts);

    assert_eq!(stats.total_usage, usage);
    assert_eq!(stats.total_turns, 4);
    assert!(stats.duration_ms >= 50);
    assert_eq!(stats.duration_api_ms, 37);
    assert_eq!(stats.cost_tracker.total_api_calls, 1);
    assert_eq!(stats.permission_denials.len(), 1);
    assert_eq!(stats.permission_denials[0].tool_use_id, "toolu_denied");
}
