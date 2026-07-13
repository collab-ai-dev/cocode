use crate::session_runtime::SessionHandle;

use super::{SessionControlError, require_runtime};

pub async fn set_fast_mode(
    runtime: Option<SessionHandle>,
    active: bool,
) -> Option<coco_types::SessionId> {
    let runtime = runtime?;
    runtime.set_fast_mode(active).await;
    Some(runtime.session_id().clone())
}

pub async fn next_fast_mode_state(
    runtime: Option<SessionHandle>,
) -> Result<bool, SessionControlError> {
    let runtime = require_runtime(runtime, "fast mode toggle")?;
    let cfg = runtime.current_engine_config().await;
    let requested = !cfg.fast_mode;
    Ok(requested && coco_config::is_fast_mode_supported_by_model(&cfg.model_id))
}
