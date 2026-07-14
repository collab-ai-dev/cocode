use tracing::info;

use super::session_control_error;
use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_controls;

/// `control/updateEnv` — accept environment variable updates.
///
/// Passing an empty string for a value is interpreted as "unset" and
/// counted as a clear. When a `SessionRuntime` is installed, updates are
/// applied to the runtime-owned shell env store consumed by future shell
/// tool spawns. The no-runtime fallback still acknowledges updates for
/// protocol compatibility, but has no shell provider to update.
pub(crate) async fn handle_update_env(
    params: coco_types::UpdateEnvParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let result = match session_controls::update_env(
        ctx.resolve_runtime().await,
        ctx.active_session_id().await,
        params.env,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return session_control_error(error),
    };
    info!(
        session_id = %result.session_id,
        applied = result.applied,
        cleared = result.cleared,
        "AppServerHost: control/updateEnv"
    );
    HandlerResult::ok_empty()
}
