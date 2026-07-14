use crate::session_runtime::SessionHandle;

use super::{SessionControlError, require_runtime};

pub async fn session_cost(
    runtime: Option<SessionHandle>,
) -> Result<coco_types::SessionCostResult, SessionControlError> {
    let runtime = require_runtime(runtime, "session/cost")?;
    let usage = runtime.session_usage_snapshot().await;
    let text = coco_messages::format_session_cost(&usage);
    Ok(coco_types::SessionCostResult { text, usage })
}

pub async fn session_status(
    runtime: Option<SessionHandle>,
) -> Result<coco_types::SessionStatusResult, SessionControlError> {
    let runtime = require_runtime(runtime, "session/status")?;
    let text = runtime.status_report().await;
    Ok(coco_types::SessionStatusResult { text })
}

pub async fn context_usage(
    runtime: Option<SessionHandle>,
    has_active_session: bool,
) -> Result<coco_types::ContextUsageResult, SessionControlError> {
    let Some(runtime) = runtime else {
        if has_active_session {
            return Err(SessionControlError::ActiveRuntimeRequired {
                operation: "context usage",
            });
        }
        return Err(SessionControlError::NoActiveSession);
    };
    runtime
        .analyze_main_context()
        .await
        .map(|report| report.to_wire())
        .map_err(|error| SessionControlError::ContextUsage(error.to_string()))
}
