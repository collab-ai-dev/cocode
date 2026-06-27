//! Resolve `--resume` / `--continue` / `--fork-session` CLI flags
//! into a concrete `ResumePlan` (source session id, prior messages,
//! and the live session id the new turn should write under).
//!
//! Keeping the flag-resolution logic in one place lets `main.rs`,
//! `tui_runner.rs`, and `sdk_server::sdk_runner` all reuse it without
//! duplicating the "id vs jsonl path vs --continue most-recent" rules.
//!
//! The resolver is filesystem-only; it never touches model runtimes,
//! `SessionManager`, or runtime state. Callers thread the resulting
//! `ResumePlan` into either:
//! - `RunChatOptions::prior_messages` (headless path) +
//!   `ResumePlan::session_id` for the runtime config, or
//! - `runtime.history.lock().await = plan.prior_messages` before
//!   spawning the TUI driver.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use coco_session::TranscriptStore;
use coco_session::recovery::ConversationForResume;
use coco_session::recovery::can_resume_session;
use coco_session::recovery::fork_conversation;
use coco_session::recovery::load_conversation_for_resume;
use uuid::Uuid;

use crate::Cli;

/// Result of resolving the resume-related CLI flags.
#[derive(Debug)]
pub struct ResumePlan {
    /// The session id that the upcoming run should write transcript
    /// entries under. For `--resume` / `--continue` this is the
    /// source session id (writes append onto the existing JSONL).
    /// For `--fork-session` this is a fresh uuid (writes go into a
    /// new JSONL that begins with a copy of the source).
    pub session_id: String,
    /// Source session id we loaded messages from. Same as
    /// `session_id` for resume/continue; different for fork.
    pub source_session_id: String,
    /// Path to the source transcript JSONL.
    pub source_path: PathBuf,
    /// Path to the destination transcript JSONL (= source for
    /// resume/continue, fresh file for fork).
    pub destination_path: PathBuf,
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
/// - `--fork-session`: requires `--resume <id>`; copies the source
///   JSONL into `<dest_session_id>.jsonl` (where `dest` is
///   `--session-id` if provided, else a fresh uuid).
pub fn resolve(cli: &Cli, memory_base: &Path, cwd: &Path) -> Result<Option<ResumePlan>> {
    // The destination store is always the current project — fork
    // outputs land in the cwd-scoped project dir even when the
    // source lives in a different project (legitimate
    // "fork-into-this-repo" workflow).
    let dest_paths = Arc::new(coco_paths::ProjectPaths::new(
        memory_base.to_path_buf(),
        cwd,
    ));
    let dest_store = TranscriptStore::new(Arc::clone(&dest_paths));

    let (source_session_id, source_path): (String, PathBuf) =
        if let Some(arg) = cli.resume.as_deref() {
            resolve_source_arg(memory_base, cwd, &dest_store, arg)?
        } else if cli.continue_session {
            match resolve_most_recent_across_projects(memory_base)? {
                Some(s) => s,
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
                Some(s) => s,
                None => {
                    anyhow::bail!("--fork-session requires an existing session to copy from");
                }
            }
        } else {
            return Ok(None);
        };

    if !can_resume_session(&source_path) {
        anyhow::bail!(
            "transcript at {} is empty or unreadable; nothing to resume",
            source_path.display(),
        );
    }
    let conversation = load_conversation_for_resume(&source_path)
        .map_err(|e| anyhow::anyhow!("failed to load transcript {}: {e}", source_path.display()))?;

    let prior_messages = conversation.messages.clone();

    if cli.fork_session {
        let dest_id = cli
            .session_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let dest_path = dest_store.transcript_path(&dest_id);
        fork_conversation(&source_path, &dest_path, &dest_id).map_err(|e| {
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
        prior_messages,
        conversation,
        is_fork: false,
    }))
}

/// Resolve `--resume <arg>` — accepts either a bare session id or a
/// `.jsonl` path. Returns `(session_id, transcript_path)`.
///
/// For bare session ids we walk every project under
/// `<memory_base>/projects/*/`, preferring the cwd-scoped project when
/// present so
/// `--resume <id>` from inside a repo lands on that repo's session
/// even if the id exists in multiple projects.
fn resolve_source_arg(
    memory_base: &Path,
    cwd: &Path,
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

    // Bare id: look only under the current project (with sibling-
    // worktree fallback). When `dir` is set and the direct + worktree
    // probes both miss, do NOT cross over into other projects. A global
    // scan here would silently open someone else's session and write
    // follow-up turns into the wrong project dir.
    if let Some(resolved) =
        coco_session::storage::resolve_session_file_path(memory_base, arg, Some(cwd))?
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
    if latest.session_id.is_empty() {
        return Ok(None);
    }
    // Resolve back to the on-disk path via the global scan since
    // `list_all_sessions` returned bare metadata.
    let resolved =
        coco_session::storage::resolve_session_file_path(memory_base, &latest.session_id, None)?;
    let Some(resolved) = resolved else {
        // Race: file disappeared between list and resolve. Treat as
        // no recent session rather than erroring.
        return Ok(None);
    };
    Ok(Some((latest.session_id, resolved.file_path)))
}

#[cfg(test)]
#[path = "resume_resolver.test.rs"]
mod tests;
