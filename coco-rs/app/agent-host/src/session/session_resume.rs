use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct SessionResumeInput {
    pub(crate) target: coco_types::SessionTarget,
    pub(crate) plan_mode_instructions: Option<String>,
}

#[derive(Debug)]
pub(crate) struct LoadedResumeSession {
    pub(crate) session: coco_session::Session,
    pub(crate) session_id: coco_types::SessionId,
    pub(crate) conversation: coco_session::recovery::ConversationForResume,
}

#[derive(Debug)]
pub(crate) enum LoadResumeSessionError {
    InvalidRequest(String),
    Internal(String),
}

impl LoadResumeSessionError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::InvalidRequest(message) | Self::Internal(message) => message,
        }
    }
}

pub(crate) async fn load_resume_session(
    manager: Option<Arc<coco_session::SessionManager>>,
    input: SessionResumeInput,
) -> Result<LoadedResumeSession, LoadResumeSessionError> {
    let Some(manager) = manager else {
        return Err(LoadResumeSessionError::InvalidRequest(
            "session persistence is not enabled on this server".to_string(),
        ));
    };
    let memory_base = manager.memory_base().to_path_buf();
    let manager_arc = Arc::clone(&manager);
    let target_id = input.target.session_id.as_str().to_string();
    let resume_result = tokio::task::spawn_blocking(move || manager_arc.resume(&target_id)).await;
    let session = match resume_result {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            return Err(LoadResumeSessionError::InvalidRequest(format!(
                "session/resume: {error}"
            )));
        }
        Err(join_err) => {
            return Err(LoadResumeSessionError::Internal(format!(
                "session/resume task panicked: {join_err}"
            )));
        }
    };
    let session_id = coco_types::SessionId::try_new(session.id.clone()).map_err(|error| {
        LoadResumeSessionError::InvalidRequest(format!(
            "session/resume: invalid persisted session id: {error}"
        ))
    })?;

    // The transcript lives in the resumed session's own project tree. Route
    // through SessionManager's storage resolver so linked worktrees with
    // different slug suffixes still resolve to the right JSONL file.
    let transcript_path = coco_session::storage::resolve_session_file_path(
        &memory_base,
        &session.id,
        Some(&session.working_dir),
    )
    .ok()
    .flatten()
    .map(|resolved| resolved.file_path);
    let Some(transcript_path) = transcript_path.as_ref() else {
        return Err(LoadResumeSessionError::InvalidRequest(format!(
            "session/resume: transcript for {} was not found",
            session.id
        )));
    };
    let conversation = coco_session::recovery::load_conversation_for_resume(transcript_path)
        .map_err(|error| {
            LoadResumeSessionError::InvalidRequest(format!(
                "session/resume: transcript load failed: {error}"
            ))
        })?;

    Ok(LoadedResumeSession {
        session,
        session_id,
        conversation,
    })
}
