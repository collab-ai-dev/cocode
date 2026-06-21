//! End-of-batch nested-memory attachment drain.
//!
//! When a tool batch finishes, this drains
//! [`coco_tool_runtime::ToolUseContext::nested_memory_attachment_triggers`]
//! (the Set populated by `Read`/`NotebookEdit`/etc. via
//! [`coco_tools::track_nested_memory_attachment`]), runs each triggered
//! file through [`coco_context::traverse_for_file`], and stores the
//! resulting [`NestedMemoryInfo`] entries on
//! [`QueryEngine::pending_nested_memory`] for the next reminder build
//! to consume.
//!
//! Two ends of the pipeline used to be unwired:
//! 1. The trigger Set was a write-only black hole — populated by tools,
//!    never drained. This module provides the drain.
//! 2. [`crate::reminder_adapters::MemoryAdapter::nested_memories`]
//!    intentionally returns `Vec::new()` because nested-CLAUDE.md
//!    discovery is engine-driven (file-read triggers), not state-driven
//!    (memory store recall). The pending slot is the engine-side
//!    delivery channel.
//!
//! Dedup is two-gated, mirroring the TS `memoryFilesToAttachments`:
//! 1. [`QueryEngine::loaded_nested_memory_paths`] — a non-evicting set
//!    that dedups within a user-prompt cycle. The engine (and this set)
//!    are rebuilt per cycle, so it does not survive across prompts.
//! 2. The session-persistent [`coco_context::FileReadState`] — survives
//!    the per-cycle rebuild, so a CLAUDE.md already injected (or already
//!    Read by a tool) on an earlier prompt is not re-injected when a
//!    later prompt re-reads the same subtree. This is the gate the TS
//!    side spells `readFileState.has(path)`.

use std::path::PathBuf;

use coco_messages::AttachmentMessage;
use coco_messages::Message;
use coco_system_reminder::generators::memory::NestedMemoryInfo;
use coco_tool_runtime::ToolUseContext;

use crate::engine::QueryEngine;

impl QueryEngine {
    /// Drain `ctx.nested_memory_attachment_triggers`, traverse each
    /// triggered file's CWD→file slice, and append the resulting
    /// memory entries to `self.pending_nested_memory`.
    ///
    /// Idempotent across calls within a turn — the trigger Set is
    /// cleared in place, so a second drain at the same site is a no-op.
    /// Safe to call when the Set is empty (early return).
    ///
    /// `ctx.cwd_override` wins over the process cwd when set, matching
    /// the worktree-isolated subagent contract.
    pub(crate) async fn drain_nested_memory_triggers(&self, ctx: &ToolUseContext) {
        let triggered_paths: Vec<PathBuf> = {
            let mut triggers = ctx.nested_memory_attachment_triggers.write().await;
            if triggers.is_empty() {
                return;
            }
            // `drain` empties the Set in place; the local Vec is cheap (Strings move, no copy).
            triggers.drain().map(PathBuf::from).collect()
        };

        let cwd = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok());
        let Some(cwd) = cwd else {
            // No cwd anchor → traversal can't compute the CWD↔file
            // slice. Skip silently; trigger paths are already drained
            // so the next batch starts clean.
            return;
        };

        // Session-level dedup. Held across the whole drain so
        // sibling triggers (e.g. reading two files in the same subtree)
        // share the loaded set within one batch.
        // Instruction-injection guard: only traverse nested memory for trigger
        // files inside an allowed working root (cwd or an additional dir). A
        // file the model Read elsewhere on disk must not pull in arbitrary
        // CLAUDE.md.
        let cwd_str = cwd.to_string_lossy().into_owned();
        let allowed_dirs: Vec<String> = ctx
            .permission_context
            .additional_dirs
            .keys()
            .cloned()
            .collect();

        // Gate 2 (cross-cycle): snapshot the session-persistent
        // FileReadState keys. `loaded_nested_memory_paths` is rebuilt per
        // prompt cycle and can't suppress a CLAUDE.md shown on an earlier
        // prompt; FileReadState survives the rebuild, so a memory file it
        // already tracks (prior injection or a direct tool Read) is skipped.
        // Snapshot once (LRU-capped, ≤100 paths) to avoid holding the FRS
        // lock across traversal. Mirrors TS `readFileState.has(path)`.
        let frs_seen: std::collections::HashSet<PathBuf> = match self.file_read_state.as_ref() {
            Some(frs_arc) => frs_arc
                .read()
                .await
                .iter_entries()
                .map(|(p, _)| p.to_path_buf())
                .collect(),
            None => std::collections::HashSet::new(),
        };

        let mut loaded = self.loaded_nested_memory_paths.lock().await;
        let mut new_entries: Vec<NestedMemoryInfo> = Vec::new();
        let mut newly_loaded: Vec<(String, String, coco_context::MemoryFileSource)> = Vec::new();
        let mut frs_records: Vec<(PathBuf, String, bool)> = Vec::new();
        for path in triggered_paths {
            if !coco_permissions::is_path_within_allowed_dirs(
                &path.to_string_lossy(),
                &cwd_str,
                &allowed_dirs,
            ) {
                continue;
            }
            let trigger_path = path.display().to_string();
            let entries = coco_context::traverse_for_file(&path, &cwd, &mut loaded);
            for entry in entries {
                // Gate 2: a prior tool Read or an earlier-cycle injection
                // already surfaced this memory file — don't re-inject it.
                if frs_seen.contains(&entry.path) {
                    continue;
                }
                newly_loaded.push((
                    entry.path.display().to_string(),
                    trigger_path.clone(),
                    entry.source,
                ));
                let content_differs_from_disk = entry.content != entry.raw_content;
                frs_records.push((
                    entry.path.clone(),
                    entry.raw_content.clone(),
                    content_differs_from_disk,
                ));
                new_entries.push(NestedMemoryInfo {
                    path: entry.path.display().to_string(),
                    content: entry.content,
                });
            }
        }
        drop(loaded);

        // Record injected memory files in FileReadState so `detect_changed_files`
        // surfaces mid-session edits to auto-injected CLAUDE.md/AGENTS.md.
        // Skip files a prior tool read already tracks so we don't clobber
        // a partial-view entry. mtimes are computed outside the lock so we
        // never await while holding the write guard.
        if let Some(frs_arc) = self.file_read_state.as_ref() {
            let mut to_set: Vec<(PathBuf, String, bool, i64)> =
                Vec::with_capacity(frs_records.len());
            for (path, raw_content, content_differs_from_disk) in frs_records {
                let mtime_ms = coco_context::file_mtime_ms(&path).await.unwrap_or(0);
                to_set.push((path, raw_content, content_differs_from_disk, mtime_ms));
            }
            let mut frs = frs_arc.write().await;
            for (path, raw_content, content_differs_from_disk, mtime_ms) in to_set {
                if frs.peek(&path).is_some() {
                    continue;
                }
                let entry = if content_differs_from_disk {
                    coco_context::FileReadEntry::injected_partial(
                        raw_content,
                        mtime_ms,
                        coco_context::FileReadRange::Full,
                    )
                } else {
                    coco_context::FileReadEntry::full_real(raw_content, mtime_ms)
                };
                frs.set(path, entry);
            }
        }

        // Fire `InstructionsLoaded` for each newly-loaded memory file.
        // Reason `nested_traversal` matches the lazy traversal path here;
        // the eager pass at session start fires with `session_start`.
        if let Some(registry) = self.hooks.as_ref() {
            let ctx = self.orchestration_ctx();
            if !ctx.disable_all_hooks {
                for (loaded_path, trigger_path, source) in newly_loaded {
                    let memory_type = match source {
                        coco_context::MemoryFileSource::Managed => {
                            coco_hooks::orchestration::MemoryType::Managed
                        }
                        coco_context::MemoryFileSource::UserGlobal => {
                            coco_hooks::orchestration::MemoryType::User
                        }
                        coco_context::MemoryFileSource::Local => {
                            coco_hooks::orchestration::MemoryType::Local
                        }
                        coco_context::MemoryFileSource::ProjectConfig
                        | coco_context::MemoryFileSource::Project => {
                            coco_hooks::orchestration::MemoryType::Project
                        }
                    };
                    if let Err(e) = coco_hooks::orchestration::execute_instructions_loaded(
                        registry,
                        &ctx,
                        &loaded_path,
                        memory_type,
                        coco_hooks::orchestration::InstructionsLoadReason::NestedTraversal,
                        /*globs*/ None,
                        Some(trigger_path.as_str()),
                        /*parent_file_path*/ None,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, path = %loaded_path, "InstructionsLoaded hook failed");
                    }
                }
            }
        }

        if new_entries.is_empty() {
            return;
        }
        let mut pending = self.pending_nested_memory.lock().await;
        pending.extend(new_entries);
    }

    pub(crate) async fn drain_dynamic_skill_triggers(
        &self,
        ctx: &ToolUseContext,
        history: &mut coco_messages::MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    ) {
        // Check source FIRST. Without it we can't dispatch — draining
        // the trigger sets unconditionally would lose paths (HashSets
        // de-dupe future tool calls to the same files/ancestry).
        let Some(source) = self.reminder_sources.skills.as_ref().cloned() else {
            return;
        };

        let triggered_dirs: Vec<PathBuf> = {
            let mut triggers = ctx.dynamic_skill_dir_triggers.write().await;
            triggers.drain().map(PathBuf::from).collect()
        };
        let triggered_paths: Vec<PathBuf> = {
            let mut triggers = ctx.dynamic_skill_path_triggers.write().await;
            triggers.drain().map(PathBuf::from).collect()
        };

        if triggered_dirs.is_empty() && triggered_paths.is_empty() {
            return;
        }

        let cwd = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        // (1) Nested-dir discovery: each new `.coco/skills/` dir
        // surfaces a model-visible dynamic_skill attachment listing
        // the newly-loaded skill names.
        for dir in triggered_dirs {
            if let Some(payload) = source.load_dynamic_skill_dir(&dir, &cwd).await {
                crate::history_sync::history_push_and_emit(
                    history,
                    Message::Attachment(AttachmentMessage::silent_dynamic_skill(payload)),
                    event_tx,
                )
                .await;
            }
        }

        // (2) Conditional-skill activation: promote any path-gated
        // skills whose `paths` patterns match a file the batch touched.
        // Promotion alone is enough — the next `skill_listing` reminder
        // turn will surface newly-visible names via `take_unannounced_skills`
        // delta, so we don't emit a separate attachment here.
        if !triggered_paths.is_empty() {
            let activated = source
                .activate_skills_for_paths(&triggered_paths, &cwd)
                .await;
            if !activated.is_empty() {
                tracing::debug!(
                    activated = ?activated,
                    "activated conditional skills via path triggers"
                );
            }
        }
    }

    /// Take and clear the engine-side pending nested-memory slot.
    ///
    /// Called once per turn from
    /// [`crate::engine_turn_reminders`] right before building
    /// `TurnReminderInput`. Returning `Vec::new()` (the common case
    /// when no triggers fired this turn) is a no-op for the generator,
    /// which short-circuits on empty input.
    pub(crate) async fn take_pending_nested_memory(&self) -> Vec<NestedMemoryInfo> {
        let mut pending = self.pending_nested_memory.lock().await;
        std::mem::take(&mut *pending)
    }

    /// Reset the session-level dedup set. Wired to `/clear` /
    /// conversation-reset paths so a fresh conversation re-injects
    /// memory files even if their content is unchanged.
    #[allow(dead_code)] // wired by /clear paths added in a follow-up
    pub(crate) async fn clear_loaded_nested_memory_paths(&self) {
        self.loaded_nested_memory_paths.lock().await.clear();
    }
}

#[cfg(test)]
#[path = "engine_attachments.test.rs"]
mod tests;
