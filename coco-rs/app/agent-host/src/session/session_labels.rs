use crate::session_rename::RenamePersistenceError;
use crate::session_runtime::SessionHandle;

#[derive(Debug, thiserror::Error)]
pub enum SessionLabelError {
    #[error("session rename failed")]
    Rename(RenamePersistenceError),
    #[error("session rename auto-generation requires an active runtime")]
    AutoRenameRuntimeRequired,
    #[error("session rename auto-generation failed")]
    AutoRename(crate::session_rename::AutoRenameError),
    #[error("session/toggleTag requires a non-empty tag")]
    EmptyTag,
    #[error("session/toggleTag failed for {session_id}: {source}")]
    ToggleTag {
        session_id: coco_types::SessionId,
        source: anyhow::Error,
    },
}

impl From<RenamePersistenceError> for SessionLabelError {
    fn from(error: RenamePersistenceError) -> Self {
        Self::Rename(error)
    }
}

impl SessionLabelError {
    pub fn user_message(&self) -> String {
        match self {
            Self::Rename(error) => error.user_message(),
            Self::AutoRenameRuntimeRequired => "Cannot rename: no active session runtime".into(),
            Self::AutoRename(error) => error.user_message().to_string(),
            Self::EmptyTag => self.to_string(),
            Self::ToggleTag { .. } => self.to_string(),
        }
    }
}

pub async fn resolve_rename_name(
    runtime: Option<&SessionHandle>,
    request: coco_commands::ParsedRename,
) -> Result<String, SessionLabelError> {
    match request {
        coco_commands::ParsedRename::Explicit(name) => Ok(name),
        coco_commands::ParsedRename::Auto => {
            let runtime = runtime.ok_or(SessionLabelError::AutoRenameRuntimeRequired)?;
            crate::session_rename::auto_generate_session_name(runtime)
                .await
                .map_err(SessionLabelError::AutoRename)
        }
    }
}

pub async fn rename_session(
    runtime: &SessionHandle,
    name: String,
) -> Result<coco_types::SessionRenameResult, SessionLabelError> {
    let name = crate::session_rename::persist_resolved_rename(runtime, name).await?;
    Ok(coco_types::SessionRenameResult { name })
}

pub async fn toggle_session_tag(
    runtime: &SessionHandle,
    tag: String,
) -> Result<coco_types::SessionToggleTagResult, SessionLabelError> {
    let tag = normalize_tag(tag)?;
    let fallback_session_id = runtime.session_id().clone();
    let (_session_id, added) =
        runtime
            .toggle_tag(tag.clone())
            .await
            .map_err(|source| SessionLabelError::ToggleTag {
                session_id: fallback_session_id,
                source,
            })?;
    Ok(coco_types::SessionToggleTagResult { tag, added })
}

fn normalize_tag(tag: String) -> Result<String, SessionLabelError> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err(SessionLabelError::EmptyTag);
    }
    Ok(tag)
}
