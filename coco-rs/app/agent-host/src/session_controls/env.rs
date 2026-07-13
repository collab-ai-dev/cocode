use std::collections::HashMap;

use crate::session_runtime::SessionHandle;

use super::SessionControlError;

pub struct EnvUpdateResult {
    pub session_id: coco_types::SessionId,
    pub applied: i32,
    pub cleared: i32,
}

pub async fn update_env(
    runtime: Option<SessionHandle>,
    active_session_id: Option<coco_types::SessionId>,
    env: HashMap<String, String>,
) -> Result<EnvUpdateResult, SessionControlError> {
    if let Some(runtime) = runtime {
        let session_id = runtime.session_id().clone();
        let (applied, cleared) = runtime.apply_session_env_updates(env);
        return Ok(EnvUpdateResult {
            session_id,
            applied,
            cleared,
        });
    }
    let Some(session_id) = active_session_id else {
        return Err(SessionControlError::NoActiveSession);
    };
    let mut applied = 0_i32;
    let mut cleared = 0_i32;
    for (_key, value) in env {
        if value.is_empty() {
            cleared += 1;
        } else {
            applied += 1;
        }
    }
    Ok(EnvUpdateResult {
        session_id,
        applied,
        cleared,
    })
}
