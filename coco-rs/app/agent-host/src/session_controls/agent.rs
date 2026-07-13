use crate::session_runtime::SessionHandle;

use super::{SessionControlError, require_runtime};

pub async fn set_agent_color(
    runtime: Option<SessionHandle>,
    color: Option<coco_types::AgentColorName>,
) -> Result<(), SessionControlError> {
    let runtime = require_runtime(runtime, "control/setAgentColor")?;
    runtime.set_agent_color(color).await;
    Ok(())
}
