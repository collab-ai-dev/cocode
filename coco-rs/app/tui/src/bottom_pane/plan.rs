//! Plan-mode prompt behavior: PlanEntry (enter plan mode) and PlanApproval
//! (teammate plan review). Exiting plan mode runs through the `ExitPlanMode`
//! permission prompt (choice list), not a dedicated prompt here.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::state::AppState;

/// Approve a PlanEntry prompt: flip into Plan mode.
pub(crate) async fn approve_plan_entry(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    state.toggle_plan_mode();
    let _ = command_tx
        .send(UserCommand::SetPermissionMode {
            mode: state.session.permission_mode,
        })
        .await;
}

/// Confirm (Enter) a PlanApproval prompt: ship the focused approve/reject
/// decision for the teammate's plan.
pub(crate) async fn confirm_plan_approval(
    p: &crate::state::PlanApprovalPromptState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    let _ = command_tx
        .send(UserCommand::PlanApprovalResponse {
            request_id: p.request_id.clone(),
            teammate_agent: p.from.clone(),
            approved: p.is_approve_focused(),
            feedback: None,
        })
        .await;
}
