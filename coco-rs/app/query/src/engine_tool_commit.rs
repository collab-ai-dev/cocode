//! Streaming tool outcome commit helpers for the session loop.

use std::future::Future;

use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_tool_runtime::StreamingHandle;
use coco_tool_runtime::call_plan::PreparedToolCall;
use coco_tool_runtime::call_plan::RunOneRuntime;
use coco_tool_runtime::call_plan::ToolCallOutcome;
use coco_tool_runtime::call_plan::UnstampedToolCallOutcome;

use crate::emit::emit_stream;
use crate::engine::RunArtifacts;
use crate::engine_helpers::extract_streaming_result_text;

pub(crate) enum StreamingCommitMode {
    CommitFlush,
    TerminalDrain,
}

pub(crate) struct StreamingCommitResult {
    pub(crate) prevent_continuation: Option<String>,
}

struct PendingCommit {
    ordered_messages: Vec<Message>,
    call_id: String,
    tool_name: String,
    output: String,
    is_error: bool,
}

pub(crate) async fn commit_streaming_tool_outcomes<F, Fut>(
    handle: StreamingHandle<F, Fut>,
    mode: StreamingCommitMode,
    history: &mut MessageHistory,
    event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    run_artifacts: &mut RunArtifacts,
) -> StreamingCommitResult
where
    F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = UnstampedToolCallOutcome> + Send + 'static,
{
    let mut prevent_continuation: Option<String> = None;
    let mut commits: Vec<PendingCommit> = Vec::new();

    match mode {
        StreamingCommitMode::CommitFlush => {
            handle
                .commit_flush(0, |outcome| {
                    collect_outcome(
                        outcome,
                        &mut commits,
                        run_artifacts,
                        Some(&mut prevent_continuation),
                    );
                })
                .await;
        }
        StreamingCommitMode::TerminalDrain => {
            handle
                .terminal_drain(0, |outcome| {
                    collect_outcome(outcome, &mut commits, run_artifacts, None);
                })
                .await;
        }
    }

    for commit in commits {
        for msg in commit.ordered_messages {
            crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
        }
        let _ = emit_stream(
            event_tx,
            crate::AgentStreamEvent::ToolUseCompleted {
                call_id: commit.call_id,
                name: commit.tool_name,
                output: commit.output,
                is_error: commit.is_error,
            },
        )
        .await;
    }

    StreamingCommitResult {
        prevent_continuation,
    }
}

fn collect_outcome(
    outcome: ToolCallOutcome,
    commits: &mut Vec<PendingCommit>,
    run_artifacts: &mut RunArtifacts,
    prevent_continuation: Option<&mut Option<String>>,
) {
    let call_id = outcome.tool_use_id().to_string();
    let tool_name = outcome.tool_id().to_string();
    let is_error = outcome.error_kind().is_some();
    let output = extract_streaming_result_text(outcome.ordered_messages());
    if let (Some(reason), Some(prevent_continuation)) =
        (outcome.prevent_continuation(), prevent_continuation)
        && prevent_continuation.is_none()
    {
        *prevent_continuation = Some(reason.to_string());
    }
    let parts = outcome.into_parts();
    if matches!(
        parts.tool_id,
        coco_types::ToolId::Builtin(coco_types::ToolName::StructuredOutput)
    ) {
        run_artifacts.structured_output_attempts =
            run_artifacts.structured_output_attempts.saturating_add(1);
    }
    if let Some(data) = parts.structured_output.clone() {
        run_artifacts.structured_output = Some(data);
    }
    commits.push(PendingCommit {
        ordered_messages: parts.ordered_messages,
        call_id,
        tool_name,
        output,
        is_error,
    });
}
