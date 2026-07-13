//! Runtime-scoped resume resolution for interactive session switching.
//!
//! This layer resolves a user-provided resume target against the live
//! runtime's session store and project root, then returns a [`ResumePlan`]
//! ready for the surface to apply. UI surfaces still own prompting and error
//! display; session lookup, project validation, and transcript loading live
//! here.

use std::sync::Arc;

use crate::resume_resolver::ResumePlan;
use crate::session_runtime::SessionHandle;

pub async fn load_resume_plan_for_runtime_target(
    session: &SessionHandle,
    target: &str,
) -> anyhow::Result<ResumePlan> {
    let manager = session.session_manager_handle();
    let target = target.to_string();
    let runtime_project_root = session.project_root().clone();
    tokio::task::spawn_blocking(move || {
        let session = match manager.resume(&target) {
            Ok(session) => session,
            Err(id_err) => {
                resolve_resume_target_by_title(&manager, &target, &runtime_project_root, &id_err)?
            }
        };
        let session_project_root = crate::paths::resolve_project_root(&session.working_dir);
        if session_project_root != runtime_project_root {
            anyhow::bail!(
                "session {} lives under project {} but this runtime is at {}; \
                 cross-project /resume is not supported. cd to the source \
                 project and try again.",
                session.id,
                session_project_root.display(),
                runtime_project_root.display(),
            );
        }
        let transcript_path =
            coco_session::TranscriptStore::new(crate::paths::project_paths(&session.working_dir))
                .transcript_path(&session.id);
        if !coco_session::recovery::can_resume_session(&transcript_path) {
            anyhow::bail!(
                "transcript at {} is empty or unreadable; nothing to resume",
                transcript_path.display()
            );
        }
        let conversation = coco_session::recovery::load_conversation_for_resume(&transcript_path)?;
        let prior_messages = conversation.messages.clone();
        let session_id = coco_types::SessionId::try_new(session.id.clone())
            .map_err(|e| anyhow::anyhow!("invalid session id '{}': {e}", session.id))?;
        Ok(ResumePlan {
            session_id: session_id.clone(),
            source_session_id: session_id,
            source_path: transcript_path.clone(),
            destination_path: transcript_path,
            cwd: session.working_dir,
            prior_messages,
            conversation,
            is_fork: false,
        })
    })
    .await
    .map_err(|err| anyhow::anyhow!("resume task failed: {err}"))?
}

pub async fn fork_resume_plan_for_runtime_session(
    session: &SessionHandle,
) -> anyhow::Result<ResumePlan> {
    let source_id = session.session_id().clone();
    if source_id.as_str().is_empty() {
        anyhow::bail!("No active session to branch from.");
    }
    let working_dir = session.original_cwd().clone();
    let memory_base = session.session_manager_handle().memory_base().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let store = coco_session::TranscriptStore::new(std::sync::Arc::new(
            coco_paths::ProjectPaths::new(memory_base, &working_dir),
        ));
        let source_path = store.transcript_path(source_id.as_str());
        if !coco_session::recovery::can_resume_session(&source_path) {
            anyhow::bail!(
                "nothing to branch yet - send a message first so there's a conversation to fork"
            );
        }
        let dest_id = coco_types::SessionId::generate();
        let dest_path = store.transcript_path(dest_id.as_str());
        coco_session::recovery::fork_conversation(&source_path, &dest_path, dest_id.as_str())
            .map_err(|error| {
                anyhow::anyhow!(
                    "fork copy {} -> {} failed: {error}",
                    source_path.display(),
                    dest_path.display()
                )
            })?;
        let conversation = coco_session::recovery::load_conversation_for_resume(&dest_path)?;
        let prior_messages = conversation.messages.clone();
        Ok(ResumePlan {
            session_id: dest_id,
            source_session_id: source_id,
            source_path,
            destination_path: dest_path,
            cwd: working_dir,
            prior_messages,
            conversation,
            is_fork: true,
        })
    })
    .await
    .map_err(|err| anyhow::anyhow!("branch task failed: {err}"))?
}

pub async fn resume_plan_session_seq_watermark(plan: &ResumePlan) -> Option<i64> {
    let transcript_path = plan.destination_path.clone();
    let session_id = plan.session_id.to_string();
    tokio::task::spawn_blocking(move || {
        coco_session::storage::read_transcript_metadata_at(&transcript_path, &session_id)
            .ok()
            .and_then(|meta| meta.session_seq_watermark)
    })
    .await
    .ok()
    .flatten()
}

pub async fn hydrate_runtime_for_resume(
    session: &SessionHandle,
    session_id: &coco_types::SessionId,
    prior_messages: &[coco_messages::Message],
) {
    let messages = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();
    session
        .replace_history_with_arc_messages(messages.clone())
        .await;
    seed_resume_transcript_state(session, session_id, prior_messages).await;

    if prior_messages.is_empty() {
        return;
    }
    restore_goal_metadata_from_messages(session, &messages).await;
}

pub async fn seed_resume_transcript_state(
    session: &SessionHandle,
    session_id: &coco_types::SessionId,
    prior_messages: &[coco_messages::Message],
) {
    session
        .seed_transcript_dedup(prior_messages.iter().filter_map(|m| m.uuid().copied()))
        .await;
    session
        .seed_tool_result_replacement_state(prior_messages, session_id, None)
        .await;
}

pub async fn restore_goal_metadata_from_messages(
    session: &SessionHandle,
    messages: &[Arc<coco_messages::Message>],
) -> Option<coco_types::ActiveGoal> {
    restore_goal_metadata_from_messages_with_trust(session, messages, /*trust_rejected*/ false)
        .await
}

pub async fn restore_goal_metadata_from_messages_with_trust(
    session: &SessionHandle,
    messages: &[Arc<coco_messages::Message>],
    trust_rejected: bool,
) -> Option<coco_types::ActiveGoal> {
    if messages.is_empty() {
        return None;
    }
    session
        .restore_goal_from_history(messages, trust_rejected)
        .await
}

/// Case-insensitive exact resolve of `/resume <name>` when the argument does
/// not match any session id directly.
fn resolve_resume_target_by_title(
    manager: &coco_session::SessionManager,
    target: &str,
    runtime_project_root: &std::path::Path,
    id_err: &coco_session::SessionError,
) -> anyhow::Result<coco_session::Session> {
    let mut matches = manager
        .find_by_title(target, true)?
        .into_iter()
        .filter(|s| same_project(&s.working_dir, runtime_project_root))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => anyhow::bail!("no session found for id or title '{target}': {id_err}"),
        1 => Ok(matches.remove(0)),
        n => {
            const MAX_CANDIDATES_SHOWN: usize = 5;
            let lines: Vec<String> = matches
                .iter()
                .take(MAX_CANDIDATES_SHOWN)
                .map(|s| format!("  {}  {}", s.id, s.title.as_deref().unwrap_or("(untitled)")))
                .collect();
            let more = if n > MAX_CANDIDATES_SHOWN {
                format!("\n  ...and {} more", n - MAX_CANDIDATES_SHOWN)
            } else {
                String::new()
            };
            anyhow::bail!(
                "ambiguous resume target '{target}': {n} sessions match. \
                 Re-run with a session id:\n{}{more}",
                lines.join("\n"),
            )
        }
    }
}

fn same_project(session_cwd: &std::path::Path, runtime_root: &std::path::Path) -> bool {
    crate::paths::resolve_project_root(session_cwd) == runtime_root
}

#[cfg(test)]
#[path = "runtime_resume.test.rs"]
mod tests;
