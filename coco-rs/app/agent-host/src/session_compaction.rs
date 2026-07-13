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
    let _ = event_tx
        .send(coco_types::CoreEvent::Protocol(
            coco_types::ServerNotification::TurnStarted(coco_types::TurnStartedParams {
                turn_id: turn_id.clone(),
            }),
        ))
        .await;
    let request = manual_compact_request(request);
    session
        .run_manual_compact(request, Some(event_tx.clone()), cancel)
        .await;
    let _ = event_tx
        .send(coco_types::CoreEvent::Protocol(
            coco_types::ServerNotification::TurnEnded(coco_types::TurnEndedParams::completed(
                turn_id,
                Some(coco_types::TokenUsage::default()),
                Some(coco_messages::StopReason::EndTurn),
            )),
        ))
        .await;
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
