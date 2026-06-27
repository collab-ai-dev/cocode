//! QueryResult construction shared by session-loop terminal branches.

use coco_messages::Message;
use coco_messages::MessageHistory;

use crate::QueryResult;
use crate::engine_loop_state::LoopAccumulator;
use crate::engine_loop_state::LoopConstants;
use crate::engine_loop_state::LoopTurnState;

/// Pure constructor for [`QueryResult`], factored out of the session loop.
/// Terminal callers return immediately after invoking it; the loop state is
/// borrowed so branch helpers can keep mutation ownership explicit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_query_result(
    consts: &LoopConstants,
    acc: &LoopAccumulator,
    turn_state: &LoopTurnState,
    response_text: String,
    cancelled: bool,
    budget_exhausted: bool,
    stop_reason: Option<String>,
    final_messages: Vec<std::sync::Arc<Message>>,
    final_history: MessageHistory,
) -> QueryResult {
    QueryResult {
        response_text,
        final_history,
        turns: turn_state.turn,
        total_usage: acc.total_usage,
        cost_tracker: acc.cost_tracker.clone(),
        cancelled,
        budget_exhausted,
        last_continue_reason: turn_state.transition.clone(),
        duration_ms: consts.started_at.elapsed().as_millis() as i64,
        duration_api_ms: acc.api_time_ms,
        stop_reason,
        permission_denials: acc.permission_denials.clone(),
        final_messages,
        structured_output: acc.run_artifacts.structured_output.clone(),
        max_turns_reached: acc.run_artifacts.max_turns_reached.clone(),
    }
}
