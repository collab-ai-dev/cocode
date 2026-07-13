use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::session_runtime::SessionHandle;

pub enum SummarizeRewindOutcome {
    Applied,
    TargetMissing,
    Failed,
}

pub async fn run_summarize_rewind(
    session: &SessionHandle,
    message_id: &str,
    direction: coco_messages::PartialCompactDirection,
    feedback: Option<String>,
    event_tx: Option<mpsc::Sender<coco_types::CoreEvent>>,
) -> SummarizeRewindOutcome {
    let messages = session.history_messages().await;
    let Some(pivot_index) = messages.iter().position(|message| match message.as_ref() {
        coco_messages::Message::User(user) => user.uuid.to_string() == message_id,
        _ => false,
    }) else {
        return SummarizeRewindOutcome::TargetMissing;
    };

    let engine = session.build_engine(CancellationToken::new()).await;
    let mut history = coco_messages::MessageHistory::new();
    for message in messages {
        history.push_arc(message);
    }
    let outcome = engine
        .run_partial_compact(
            &mut history,
            &event_tx,
            pivot_index,
            direction,
            feedback,
            /*custom_instructions*/ None,
        )
        .await;

    match outcome {
        coco_compact::CompactOutcome::Applied => {
            session.commit_compacted_history(history).await;
            SummarizeRewindOutcome::Applied
        }
        coco_compact::CompactOutcome::Skipped | coco_compact::CompactOutcome::Failed => {
            SummarizeRewindOutcome::Failed
        }
    }
}

pub async fn run_manual_compact_turn(
    session: SessionHandle,
    request: coco_commands::handlers::compact::CompactRequest,
    turn_id: coco_types::TurnId,
    event_tx: mpsc::Sender<coco_types::CoreEvent>,
    cancel: CancellationToken,
) {
    let started_at = std::time::Instant::now();
    let _ = event_tx
        .send(coco_types::CoreEvent::Protocol(
            coco_types::ServerNotification::TurnStarted(coco_types::TurnStartedParams {
                turn_id: turn_id.clone(),
            }),
        ))
        .await;
    let request = manual_compact_request(request);
    let outcome = session
        .run_manual_compact(request, Some(event_tx.clone()), cancel)
        .await;
    let result = manual_compact_session_result(&session, outcome, started_at.elapsed());
    let terminal = manual_compact_turn_ended(turn_id, outcome).with_session_result(result.clone());
    let _ = event_tx
        .send(coco_types::CoreEvent::Protocol(
            coco_types::ServerNotification::SessionResult(Box::new(result)),
        ))
        .await;
    let _ = event_tx
        .send(coco_types::CoreEvent::Protocol(
            coco_types::ServerNotification::TurnEnded(terminal),
        ))
        .await;
}

fn manual_compact_session_result(
    session: &SessionHandle,
    outcome: coco_compact::CompactOutcome,
    elapsed: std::time::Duration,
) -> coco_types::SessionResultParams {
    let (is_error, stop_reason, errors) = match outcome {
        coco_compact::CompactOutcome::Applied => (false, "manual_compact_applied", Vec::new()),
        coco_compact::CompactOutcome::Skipped => (false, "manual_compact_skipped", Vec::new()),
        coco_compact::CompactOutcome::Failed => (
            true,
            "manual_compact_failed",
            vec!["manual compaction failed".to_string()],
        ),
    };
    coco_types::SessionResultParams {
        session_id: session.session_id().clone(),
        total_turns: 1,
        duration_ms: elapsed.as_millis() as i64,
        duration_api_ms: 0,
        is_error,
        stop_reason: stop_reason.to_string(),
        total_cost_usd: 0.0,
        usage: coco_types::TokenUsage::default(),
        model_usage: std::collections::HashMap::new(),
        permission_denials: Vec::new(),
        result: None,
        errors,
        structured_output: None,
        fast_mode_state: None,
        num_api_calls: None,
    }
}

fn manual_compact_turn_ended(
    turn_id: coco_types::TurnId,
    outcome: coco_compact::CompactOutcome,
) -> coco_types::TurnEndedParams {
    match outcome {
        coco_compact::CompactOutcome::Applied | coco_compact::CompactOutcome::Skipped => {
            coco_types::TurnEndedParams::completed(
                turn_id,
                Some(coco_types::TokenUsage::default()),
                None,
            )
        }
        coco_compact::CompactOutcome::Failed => coco_types::TurnEndedParams::failed(
            turn_id,
            Some(coco_types::TokenUsage::default()),
            coco_types::ErrorPayload {
                message: "manual compaction failed".to_string(),
                code: coco_types::ErrorCode::Unknown,
            },
        ),
    }
}

fn manual_compact_request(
    request: coco_commands::handlers::compact::CompactRequest,
) -> coco_query::ManualCompactRequest {
    let command_args = request.custom_instructions;
    let custom_instructions = if command_args.is_empty() {
        None
    } else {
        Some(command_args.clone())
    };
    coco_query::ManualCompactRequest {
        custom_instructions,
        command_args,
    }
}
