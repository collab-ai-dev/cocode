use super::*;
/// Lite-read window: when a transcript exceeds this size, scan only
/// the first and last `LITE_READ_WINDOW` bytes rather than loading
/// the whole file (64 KB on each end). Session-picker metadata
/// (`first_prompt`, `custom_title`, `tag`, `last_prompt`, `git_branch`,
/// `cwd`) lives at the top of the transcript, while re-appended-on-exit
/// values land near the tail; 64 KB at each end captures both without
/// streaming the multi-megabyte body.
const LITE_READ_WINDOW: u64 = 64 * 1024;

/// Read lightweight metadata from a transcript file without loading all
/// messages. Scans the first and last portion of the file.
/// Public alias of the per-file lite-metadata reader so
/// `SessionManager::load` can derive a `Session` from a resolved
/// transcript path without re-walking the projects tree.
pub fn read_transcript_metadata_at(
    path: &Path,
    session_id: &str,
) -> crate::Result<TranscriptMetadata> {
    read_transcript_metadata(path, session_id)
}

pub(super) fn read_transcript_metadata(
    path: &Path,
    session_id: &str,
) -> crate::Result<TranscriptMetadata> {
    if !path.exists() {
        return Err(crate::SessionError::TranscriptNotFound {
            path: path.to_path_buf(),
        });
    }

    let file_meta = std::fs::metadata(path)?;
    let file_size = file_meta.len();

    let created_at = file_meta
        .created()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();

    let modified_at = file_meta
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();

    // For small files (≤ 2× the lite window) load everything; for
    // larger transcripts scan only the head and tail. The head pass
    // captures `first_prompt`, `cwd`, `git_branch`, and the
    // sidechain/message-count signal; the tail pass picks up the
    // metadata entries (`custom-title`, `tag`, `last-prompt`) that
    // are re-appended on exit so they survive head-truncation.
    let content = if file_size > LITE_READ_WINDOW * 2 {
        read_head_and_tail(path, LITE_READ_WINDOW)?
    } else {
        std::fs::read_to_string(path)?
    };
    let entries: Vec<Entry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse_entry)
        .collect();

    // Content-derived fields come from the shared fold; the fs-stat trio
    // (created/modified/file_size) is layered on top here — those are the
    // disk-specific bits a non-fs backend cannot supply from entries alone.
    let typed_session_id = checked_session_id(session_id)?;
    let mut meta = fold_transcript_metadata(&entries, &typed_session_id);
    meta.created_at = created_at;
    meta.modified_at = modified_at;
    meta.file_size = file_size;
    Ok(meta)
}

/// Derive the **content** fields of [`TranscriptMetadata`] from in-memory
/// entries — everything except the fs-stat trio
/// (`created_at`/`modified_at`/`file_size`), which is left at its default
/// for the caller to stamp. Pure: no IO. Shared by the disk head/tail
/// reader and any non-fs backend ([`crate::store::InMemoryStore`]) that
/// already holds the entries in memory.
pub fn fold_transcript_metadata(entries: &[Entry], session_id: &SessionId) -> TranscriptMetadata {
    let mut first_prompt = String::new();
    let mut custom_title: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut agent_name: Option<String> = None;
    let mut agent_color: Option<String> = None;
    let mut agent_setting: Option<String> = None;
    let mut mode: Option<String> = None;
    let mut worktree_state: Option<serde_json::Value> = None;
    let mut pr_link: Option<serde_json::Value> = None;
    let mut tag: Option<String> = None;
    let mut last_prompt: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut session_seq_watermark: Option<i64> = None;
    let mut is_sidechain = false;
    let mut first_message_seen = false;
    let mut message_count: i32 = 0;

    for entry in entries {
        match entry {
            Entry::Transcript(t) => {
                if t.entry_type == entry_kind::USER || t.entry_type == entry_kind::ASSISTANT {
                    message_count += 1;
                }
                if first_prompt.is_empty() && t.entry_type == entry_kind::USER {
                    let candidate = extract_text_content(t);
                    // Filters synthetic interrupt markers so the resume
                    // picker shows the user's real first prompt, not
                    // "[Request interrupted by user]". Uses literal
                    // equality against the interrupt markers from
                    // `coco-messages::creation`.
                    if !is_synthetic_first_prompt_candidate(&candidate) {
                        first_prompt = candidate;
                    }
                }
                // Session-level sidechain is derived from the FIRST message
                // only, mirroring the TS upstream (`transcript[0].isSidechain`
                // / the first-line scan in listSessionsImpl / sessionStorage).
                // coco-rs interleaves inline AgentTool subagent messages
                // (`is_sidechain = true`) into the *main* transcript, so an
                // OR-over-all fold would mis-flag any main session that ran a
                // subagent as a sidechain session and hide it from
                // `list_main_sessions` / `--continue` / auto-dream. Only a
                // transcript whose first message is a sidechain (a pure agent
                // transcript) is a sidechain session.
                if !first_message_seen {
                    first_message_seen = true;
                    is_sidechain = t.is_sidechain;
                }
                if cwd.is_none() && !t.cwd.is_empty() {
                    cwd = Some(t.cwd.clone());
                }
                if t.git_branch.is_some() {
                    git_branch.clone_from(&t.git_branch);
                }
            }
            Entry::Metadata(m) => match m {
                MetadataEntry::CustomTitle {
                    custom_title: ct, ..
                } => {
                    custom_title = Some(ct.clone());
                }
                MetadataEntry::Tag { tag: t, .. } => {
                    tag = Some(t.clone());
                }
                MetadataEntry::LastPrompt {
                    last_prompt: lp, ..
                } => {
                    last_prompt = Some(lp.clone());
                }
                MetadataEntry::AiTitle {
                    ai_title: title, ..
                } => {
                    ai_title = Some(title.clone());
                }
                MetadataEntry::AgentName {
                    agent_name: name, ..
                } => {
                    agent_name = Some(name.clone());
                }
                MetadataEntry::AgentColor {
                    session_id: entry_session_id,
                    agent_color: color,
                } if entry_session_id == session_id => {
                    agent_color = Some(color.clone());
                }
                MetadataEntry::AgentSetting {
                    session_id: entry_session_id,
                    agent_setting: setting,
                } if entry_session_id == session_id => {
                    agent_setting = Some(setting.clone());
                }
                MetadataEntry::Mode {
                    session_id: entry_session_id,
                    mode: m,
                } if entry_session_id == session_id => {
                    mode = Some(m.clone());
                }
                MetadataEntry::WorktreeState { payload }
                    if metadata_payload_session_id(payload).as_deref()
                        == Some(session_id.as_str()) =>
                {
                    worktree_state = Some(payload.clone());
                }
                MetadataEntry::PrLink { payload }
                    if metadata_payload_session_id(payload).as_deref()
                        == Some(session_id.as_str()) =>
                {
                    pr_link = Some(payload.clone());
                }
                MetadataEntry::SessionSeqWatermark {
                    session_id: entry_session_id,
                    session_seq,
                } if entry_session_id == session_id => {
                    // Max, not last: interval-persisted entries always grow,
                    // but a close-time append can race a hook append.
                    session_seq_watermark =
                        Some(session_seq_watermark.unwrap_or(0).max(*session_seq));
                }
                MetadataEntry::Summary { .. }
                | MetadataEntry::CostSummary { .. }
                | MetadataEntry::FileHistorySnapshot { .. }
                | MetadataEntry::MarbleOrigamiCommit { .. }
                | MetadataEntry::MarbleOrigamiSnapshot { .. }
                | MetadataEntry::ContentReplacement { .. }
                | MetadataEntry::ContextEpoch { .. }
                | MetadataEntry::TaskSummary { .. }
                | MetadataEntry::AttributionSnapshot { .. }
                | MetadataEntry::AgentColor { .. }
                | MetadataEntry::AgentSetting { .. }
                | MetadataEntry::PrLink { .. }
                | MetadataEntry::WorktreeState { .. }
                | MetadataEntry::Mode { .. }
                | MetadataEntry::McpToolExposure { .. }
                | MetadataEntry::GoalSnapshot { .. }
                | MetadataEntry::GoalCleared { .. }
                | MetadataEntry::SessionSeqWatermark { .. } => {}
            },
            Entry::Unknown(_) => {}
        }
    }

    TranscriptMetadata {
        session_id: session_id.clone(),
        first_prompt,
        message_count,
        custom_title,
        ai_title,
        agent_name,
        agent_color,
        agent_setting,
        mode,
        worktree_state,
        pr_link,
        tag,
        last_prompt,
        git_branch,
        cwd,
        session_seq_watermark,
        is_sidechain,
        created_at: String::new(),
        modified_at: String::new(),
        file_size: 0,
    }
}

pub(super) fn metadata_payload_session_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

/// Read the first `window` bytes and the last `window` bytes of
/// `path`, joining them with a single newline. Drops any partial
/// JSONL lines at the seams (the byte right after the head window
/// and the byte at the start of the tail window may sit mid-record)
/// so the caller's `parse_entry` loop only sees complete lines.
pub(super) fn read_head_and_tail(path: &Path, window: u64) -> crate::Result<String> {
    use std::io::Read;
    use std::io::Seek;
    use std::io::SeekFrom;

    let mut file = std::fs::File::open(path)?;
    let total = file.metadata()?.len();

    let head_len = window.min(total);
    let mut head_buf = vec![0u8; head_len as usize];
    file.read_exact(&mut head_buf)?;
    // Truncate at the last newline so we don't carry a partial
    // record into `parse_entry` (which would surface as `Unknown`).
    if let Some(idx) = find_last_newline(&head_buf) {
        head_buf.truncate(idx);
    }

    let tail_len = window.min(total.saturating_sub(head_len));
    let mut tail_buf = vec![0u8; tail_len as usize];
    if tail_len > 0 {
        file.seek(SeekFrom::End(-(tail_len as i64)))?;
        file.read_exact(&mut tail_buf)?;
        // Skip leading partial line (everything up to the first '\n').
        if let Some(idx) = tail_buf.iter().position(|b| *b == b'\n') {
            tail_buf.drain(..=idx);
        } else {
            // No newline in the tail window — every byte belongs to a
            // single oversized line; drop it.
            tail_buf.clear();
        }
    }

    let mut combined = Vec::with_capacity(head_buf.len() + 1 + tail_buf.len());
    combined.extend_from_slice(&head_buf);
    combined.push(b'\n');
    combined.extend_from_slice(&tail_buf);
    String::from_utf8(combined)
        .map_err(|e| crate::SessionError::generic(format!("transcript not utf-8: {e}")))
}

/// Index of the rightmost newline byte in `buf`, or `None` when no
/// newline is present. Used by [`read_head_and_tail`] to drop partial
/// records at the head/tail seams.
pub(super) fn find_last_newline(buf: &[u8]) -> Option<usize> {
    buf.iter().rposition(|b| *b == b'\n')
}

/// List all transcript sessions from a directory, newest first.
pub(super) fn list_transcript_sessions(
    sessions_dir: &Path,
) -> crate::Result<Vec<TranscriptMetadata>> {
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if session_id.is_empty() {
                continue;
            }
            match read_transcript_metadata(&path, &session_id) {
                Ok(meta) => results.push(meta),
                Err(_) => {
                    // Skip corrupt / unreadable files.
                    continue;
                }
            }
        }
    }

    // Newest first by modified_at (descending). Parse numerically so
    // mixed-width millisecond timestamps still compare correctly.
    results.sort_by(|a, b| {
        let a_ms = a.modified_at.parse::<u128>().unwrap_or(0);
        let b_ms = b.modified_at.parse::<u128>().unwrap_or(0);
        b_ms.cmp(&a_ms)
    });
    Ok(results)
}

// ---------------------------------------------------------------------------
// Cross-project enumeration & worktree-aware lookup
// ---------------------------------------------------------------------------

/// Result of [`resolve_session_file_path`] — the transcript file
/// found plus the project path (or worktree path) it lives under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSessionFile {
    /// Absolute path to `<session_id>.jsonl`.
    pub file_path: PathBuf,
    /// The project root associated with the file. For a direct
    /// match this is `cwd_hint`; for a worktree fallback it's the
    /// sibling worktree path; for the global scan branch
    /// (`cwd_hint == None`) this is `None`.
    pub project_path: Option<PathBuf>,
}

/// Locate the transcript file for `session_id`.
/// Resolution order:
/// 1. **Direct project lookup**: if `cwd_hint` is `Some`, compute
/// `ProjectPaths` from that exact cwd and check `<project_dir>/<sid>.jsonl`.
/// 2. **Worktree fallback**: if step 1 missed, shell out to
/// `git worktree list --porcelain` from `cwd_hint`, slug each
/// sibling worktree, and probe each one.
/// 3. **Global scan**: when `cwd_hint` is `None`, walk
/// `<memory_base>/projects/*/` and return the first project that
/// contains the transcript. Used by SDK callers without a cwd.
/// Returns `Ok(None)` when no project has the file. I/O errors at
/// the `read_dir(<projects>)` level propagate; transient stat
/// failures on individual entries are tolerated.
pub fn resolve_session_file_path(
    memory_base: &Path,
    session_id: &str,
    cwd_hint: Option<&Path>,
) -> crate::Result<Option<ResolvedSessionFile>> {
    let filename = format!("{session_id}.jsonl");

    if let Some(cwd) = cwd_hint {
        // 1. Direct lookup at the slug for this cwd.
        let paths = ProjectPaths::new(memory_base.to_path_buf(), cwd);
        let candidate = paths.project_dir().join(&filename);
        if has_nonzero_file(&candidate) {
            return Ok(Some(ResolvedSessionFile {
                file_path: candidate,
                project_path: Some(cwd.to_path_buf()),
            }));
        }

        // 2. Worktree fallback — only fires when (a) direct miss
        // and (b) git knows about other worktrees.
        for wt in coco_git::worktree_paths(cwd) {
            if wt == cwd {
                continue;
            }
            let wt_paths = ProjectPaths::new(memory_base.to_path_buf(), &wt);
            let cand = wt_paths.project_dir().join(&filename);
            if has_nonzero_file(&cand) {
                return Ok(Some(ResolvedSessionFile {
                    file_path: cand,
                    project_path: Some(wt),
                }));
            }
        }
        return Ok(None);
    }

    // 3. Global scan — walk every project directory.
    let projects_root = coco_paths::projects_root(memory_base);
    let entries = match std::fs::read_dir(&projects_root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if has_nonzero_file(&candidate) {
            return Ok(Some(ResolvedSessionFile {
                file_path: candidate,
                project_path: None,
            }));
        }
    }
    Ok(None)
}

/// List every session transcript across **every** project under
/// `<memory_base>/projects/*/`, newest first.
/// Used by the resume picker / SDK session enumerator — callers
/// that only want this-project sessions should go through
/// [`TranscriptStore::list_sessions`] instead.
pub fn list_all_sessions(memory_base: &Path) -> crate::Result<Vec<TranscriptMetadata>> {
    let projects_root = coco_paths::projects_root(memory_base);
    let project_entries = match std::fs::read_dir(&projects_root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let mut results: Vec<TranscriptMetadata> = Vec::new();
    for project_entry in project_entries.flatten() {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        // Each project dir has the same internal layout — reuse
        // the same per-dir walker as `TranscriptStore::list_sessions`.
        if let Ok(mut found) = list_transcript_sessions(&project_dir) {
            results.append(&mut found);
        }
    }

    // Sort across all projects so newest wins overall.
    results.sort_by(|a, b| {
        let a_ms = a.modified_at.parse::<u128>().unwrap_or(0);
        let b_ms = b.modified_at.parse::<u128>().unwrap_or(0);
        b_ms.cmp(&a_ms)
    });
    Ok(results)
}

pub(super) fn has_nonzero_file(path: &Path) -> bool {
    matches!(
        std::fs::metadata(path),
        Ok(m) if m.is_file() && m.len() > 0,
    )
}
