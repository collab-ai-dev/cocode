//! Resolve `--resume` / `--continue` / `--fork-session` CLI flags
//! into a concrete `ResumePlan` (source session id, prior messages,
//! and the live session id the new turn should write under).
//!
//! Keeping the flag-resolution logic in one place lets `main.rs`,
//! `app/cli/src/tui`, and `app_server_host::SessionTurnExecutor` all reuse it without
//! duplicating the "id vs jsonl path vs --continue most-recent" rules.
//!
//! The resolver is filesystem-only; it never touches model runtimes,
//! `SessionManager`, or runtime state. Callers thread the resulting
//! `ResumePlan` into either:
//! - `RunChatOptions::prior_messages` (headless path) +
//!   `ResumePlan::session_id` for the runtime config, or
//! - `runtime.history().lock().await = plan.prior_messages` before
//!   spawning the TUI driver.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use coco_session::SessionLeaseStore;
use coco_session::TranscriptStore;
use coco_session::recovery::ConversationForResume;
use coco_session::recovery::can_resume_session;
use coco_session::recovery::fork_conversation;
use coco_session::recovery::load_conversation_for_resume;

use crate::AgentHostOptions;

/// Result of resolving the resume-related CLI flags.
#[derive(Debug)]
pub struct ResumePlan {
    /// The session id that the upcoming run should write transcript
    /// entries under. For `--resume` / `--continue` this is the
    /// source session id (writes append onto the existing JSONL).
    /// For `--fork-session` this is a fresh `SessionId` (writes go into a
    /// new JSONL that begins with a copy of the source).
    pub session_id: coco_types::SessionId,
    /// Source session id we loaded messages from. Same as
    /// `session_id` for resume/continue; different for fork.
    pub source_session_id: coco_types::SessionId,
    /// Path to the source transcript JSONL.
    pub source_path: PathBuf,
    /// Path to the destination transcript JSONL (= source for
    /// resume/continue, fresh file for fork).
    pub destination_path: PathBuf,
    /// Working directory for the runtime being constructed from this plan.
    /// Resume/continue use the source session cwd; fork uses the caller cwd
    /// because the destination transcript is rooted in the current project.
    pub cwd: PathBuf,
    /// Pre-loaded messages from the source transcript.
    pub prior_messages: Vec<coco_messages::Message>,
    /// Conversation and aggregate metadata loaded from the source transcript.
    /// Callers surface `model` and token counts in their startup
    /// banner so the user sees what they're continuing.
    pub conversation: ConversationForResume,
    /// `true` when `--fork-session` was set (the destination diverged).
    pub is_fork: bool,
}

/// Inspect the CLI flags and (when one of `--resume` / `--continue` /
/// `--fork-session` is set) load the conversation from disk.
///
/// Returns `Ok(None)` when none of the resume flags are set —
/// callers fall through to fresh-session bootstrap. Returns an error
/// when the requested source isn't on disk or the JSONL is unreadable.
///
/// Resolution rules:
/// - `--resume <id|path>`: load the named session by id, or treat
///   the argument as a path when it ends in `.jsonl`.
/// - `--continue` / `--continue-session`: load the most recent
///   non-sidechain session in `sessions_dir`.
/// - `--fork-session`: copies `--resume <id>` or the most-recent session
///   JSONL into `<dest_session_id>.jsonl` (where `dest` is
///   `--session-id` if provided, else a fresh uuid).
pub fn resolve(
    cli: &AgentHostOptions,
    memory_base: &Path,
    cwd: &Path,
) -> Result<Option<ResumePlan>> {
    // The destination store is always the current project — fork
    // outputs land in the cwd-scoped project dir even when the
    // source lives in a different project (legitimate
    // "fork-into-this-repo" workflow).
    let dest_paths = Arc::new(coco_paths::ProjectPaths::new(
        memory_base.to_path_buf(),
        cwd,
    ));
    let dest_store = TranscriptStore::new(Arc::clone(&dest_paths));

    let (source_session_id, source_path): (coco_types::SessionId, PathBuf) =
        if let Some(arg) = cli.resume.as_deref() {
            let (id, path) = resolve_source_arg(memory_base, &dest_store, arg)?;
            (
                coco_types::SessionId::try_new(id.clone())
                    .map_err(|e| anyhow::anyhow!("invalid session id '{id}': {e}"))?,
                path,
            )
        } else if cli.continue_session {
            match resolve_most_recent_across_projects(memory_base)? {
                Some((id, path)) => (
                    coco_types::SessionId::try_new(id.clone())
                        .map_err(|e| anyhow::anyhow!("invalid session id '{id}': {e}"))?,
                    path,
                ),
                None => {
                    // No prior sessions to continue. Treat as a no-op
                    // rather than an error so `coco -c` on a clean
                    // install just starts a fresh chat. Falls through
                    // to the new-session path.
                    return Ok(None);
                }
            }
        } else if cli.fork_session {
            // Fork without an explicit source: fork the most-recent.
            match resolve_most_recent_across_projects(memory_base)? {
                Some((id, path)) => (
                    coco_types::SessionId::try_new(id.clone())
                        .map_err(|e| anyhow::anyhow!("invalid session id '{id}': {e}"))?,
                    path,
                ),
                None => {
                    anyhow::bail!("--fork-session requires an existing session to copy from");
                }
            }
        } else {
            return Ok(None);
        };

    // A fork reads and rewrites the complete source JSONL. Require quiescent
    // ownership for that snapshot so an active writer cannot leave the fork
    // missing (or carrying a partial) trailing entry.
    let _fork_source_lease = if cli.fork_session {
        Some(
            dest_store
                .require_write_lease(source_session_id.as_str())
                .map_err(|error| anyhow::anyhow!(error))?,
        )
    } else {
        None
    };

    if !can_resume_session(&source_path) {
        anyhow::bail!(
            "transcript at {} is empty or unreadable; nothing to resume",
            source_path.display(),
        );
    }
    let conversation = load_conversation_for_resume(&source_path)
        .map_err(|e| anyhow::anyhow!("failed to load transcript {}: {e}", source_path.display()))?;
    let source_cwd = coco_session::storage::read_transcript_metadata_at(
        &source_path,
        source_session_id.as_str(),
    )?
    .cwd
    .filter(|cwd| !cwd.trim().is_empty())
    .map(PathBuf::from)
    .ok_or_else(|| {
        anyhow::anyhow!(
            "transcript {} has no working-directory metadata; refusing to resume in an unrelated cwd",
            source_path.display()
        )
    })?;

    let prior_messages = conversation.messages.clone();

    if cli.fork_session {
        let dest_id = match cli.session_id.clone() {
            Some(raw) => coco_types::SessionId::try_new(raw.clone())
                .map_err(|e| anyhow::anyhow!("invalid session id '{raw}': {e}"))?,
            None => coco_types::SessionId::generate(),
        };
        if dest_id == source_session_id {
            anyhow::bail!("fork destination session id must differ from the source");
        }
        // Session ids are protocol-global. Hold the global lease across the
        // existence check and no-clobber copy so concurrent forks cannot both
        // create the same id in different project directories.
        let _fork_lease = dest_store
            .require_write_lease(dest_id.as_str())
            .map_err(|error| anyhow::anyhow!(error))?;
        if coco_session::storage::resolve_session_file_path(memory_base, dest_id.as_str(), None)?
            .is_some()
        {
            anyhow::bail!("fork destination session id {dest_id} already exists");
        }
        let dest_path = dest_store.transcript_path(dest_id.as_str());
        fork_conversation(&source_path, &dest_path, dest_id.as_str(), cwd).map_err(|e| {
            anyhow::anyhow!(
                "fork copy {} → {} failed: {e}",
                source_path.display(),
                dest_path.display(),
            )
        })?;
        return Ok(Some(ResumePlan {
            session_id: dest_id,
            source_session_id,
            source_path,
            destination_path: dest_path,
            cwd: cwd.to_path_buf(),
            prior_messages,
            conversation,
            is_fork: true,
        }));
    }

    Ok(Some(ResumePlan {
        session_id: source_session_id.clone(),
        source_session_id,
        source_path: source_path.clone(),
        destination_path: source_path,
        cwd: source_cwd,
        prior_messages,
        conversation,
        is_fork: false,
    }))
}

/// Resolve `--resume <arg>` — accepts either a bare session id or a
/// `.jsonl` path. Returns `(session_id, transcript_path)`.
///
/// Bare session ids are resolved globally and must be unique. An explicit
/// `.jsonl` path is the only path-scoped form.
fn resolve_source_arg(
    memory_base: &Path,
    dest_store: &TranscriptStore,
    arg: &str,
) -> Result<(String, PathBuf)> {
    if arg.ends_with(".jsonl") {
        let path = PathBuf::from(arg);
        let abs = if path.is_absolute() {
            path
        } else {
            // Relative .jsonl path is rooted in the cwd's project
            // dir, resolving relative against the project's sessions dir.
            dest_store.project_paths().project_dir().join(&path)
        };
        let id = abs
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_default();
        return Ok((id, abs));
    }

    // A bare id is a protocol-global address. Reject legacy duplicates rather
    // than routing a writable resume based on the caller's current directory.
    if let Some(resolved) =
        coco_session::storage::resolve_session_file_path(memory_base, arg, None)?
    {
        return Ok((arg.to_string(), resolved.file_path));
    }
    anyhow::bail!(
        "no session found for id {arg} under {}",
        coco_paths::projects_root(memory_base).display(),
    );
}

/// Pick the newest non-sidechain session across **every** project.
/// For `--continue`, the resume picker walks all known projects,
/// not just the current cwd.
fn resolve_most_recent_across_projects(memory_base: &Path) -> Result<Option<(String, PathBuf)>> {
    let mut sessions = coco_session::storage::list_all_sessions(memory_base)
        .map_err(|e| anyhow::anyhow!("listing sessions failed: {e}"))?;
    // Filter out sidechains — same predicate as
    // `TranscriptStore::list_main_sessions`.
    sessions.retain(|m| !m.is_sidechain);
    if sessions.is_empty() {
        return Ok(None);
    }
    let latest = sessions.remove(0);
    let latest_session_id = latest.session_id.into_inner();
    // Resolve back to the on-disk path via the global scan since
    // `list_all_sessions` returned bare metadata.
    let resolved =
        coco_session::storage::resolve_session_file_path(memory_base, &latest_session_id, None)?;
    let Some(resolved) = resolved else {
        // Race: file disappeared between list and resolve. Treat as
        // no recent session rather than erroring.
        return Ok(None);
    };
    Ok(Some((latest_session_id, resolved.file_path)))
}

#[cfg(test)]
#[path = "resume_resolver.test.rs"]
mod tests;
