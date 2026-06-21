# coco-context

System context assembly: environment info, memory-file discovery (`CLAUDE.md` / `AGENTS.md`), attachments, file history, plan mode, mentions, prompt building.

## Key Types

- **Environment**: `EnvironmentInfo`, `Platform`, `ShellKind`, `GitStatus`, `get_environment_info`
- **Memory files (eager)**: `MemoryFile`, `MemoryFileSource` (`UserGlobal` / `ProjectConfig` / `Project` / `Local`), `discover_memory_files`. `discover_claude_md_files` retained as a hidden alias.
- **Memory filenames**: `MEMORY_FILE_CANDIDATES` (`CLAUDE.md` + `AGENTS.md`), `MEMORY_LOCAL_FILE_CANDIDATES`, `find_memory_files` — case-insensitive at every position.
- **Per-file lazy traversal**: `LoadedMemoryEntry`, `directories_to_process(file, cwd) -> (nested_dirs, cwd_level_dirs)`, `traverse_for_file(file, cwd, &mut loaded)`.
- **`.claude/rules/*.md`**: `RuleFile`, `parse_paths_field`, `collect_rule_files(rules_dir, conditional)`, `filter_rules_matching(rules, target, base_dir)`.
- **`@import` expansion**: `MAX_INCLUDE_DEPTH` (5), `TEXT_FILE_EXTENSIONS`, `extract_include_paths`, `is_text_extension`, `resolve_at_path`, `expand_imports(path, content, &mut processed, depth)`.
- **Attachments**: `Attachment`, `AttachmentBatch`, `AttachmentBudget`, `AttachmentDeduplicator`, `AttachmentSource`, `ReminderType`, `collect_batched_attachments`, `generate_all_attachments_async`
- **Plan mode**: `PlanModeAttachment`, `PlanModeExitAttachment`, `PlanWorkflow`, `Phase4Variant`, `PlanVerificationOutcome`, plan file management (`get_plan`, `set_plan_slug`, `write_plan`, `delete_plan`, `verify_plan_was_edited`, `recover_plan_for_resume`, `render_plan_mode_reminder`, `render_plan_mode_exit_reminder`, `render_auto_mode_exit_reminder`, `resolve_plans_directory`, `generate_word_slug`, etc.)
- **File history**: `FileHistoryState`, `FileHistorySnapshot`, `FileHistoryBackup`, `DiffStats`, `backup_dir`, `copy_file_history_for_resume`
- **File read state / cache**: `FileReadState`, `FileReadEntry`, `FileReadCache`, `file_mtime_ms`
- **Changed files**: `detect_changed_files`
- **Memory (legacy info type)**: `MemoryFileInfo`, `MemoryType`
- **Mentions**: `MentionResolveOptions`, `resolve_mentions`
- **User input**: `ProcessedInput`, `Mention`, `MentionType`, `process_user_input`
- **Prompt**: `SystemPrompt`, `SystemPromptBlock`, `build_system_prompt`, `build_minimal_prompt`
- **Tokens**: `estimate_tokens`, `estimate_tokens_for_messages`, `is_over_threshold`

## Module Layout

`attachment`, `changed_files`, `claude_rules`, `claudemd`, `claudemd_imports`, `environment`, `file_cache`, `file_history`, `file_read_state`, `git_operations`, `git_utils`, `memory`, `memory_filenames`, `mention_resolver`, `nested_memory`, `plan_mode`, `prompt`, `prompt_suggestion`, `suggestions`, `token_estimation`, `user_input`, `vim_mode`.

> Git worktree *creation* for agent isolation lives in `coco_coordinator::worktree` (`AgentWorktreeManager`), not here — this crate only *reads* the filesystem during memory discovery.

## Architecture

- `Platform` + `ShellKind` enums owned here (cross-crate env types).
- File history uses ordered `Vec` + content-addressed files on disk (NOT HashMap). The `FileHistorySnapshot` JSON wire shape is snake_case (`message_id`, `tracked_file_backups`, `backup_file_name`, `backup_time`) and `DateTime<Utc>` for time fields (RFC 3339 strings). See `coco-session` CLAUDE.md for the cross-crate wire policy.
- Plan mode scoped by session-local `plan_slug` for fork/resume isolation.
- Phase-4 + Interview plan workflows exposed via `settings.json` (`plan_mode.phase4_variant`, `plan_mode.workflow`) — no GrowthBook / `USER_TYPE=ant` env vars. Ultraplan (CCR web UI) intentionally skipped.

## Memory-File Pipeline

Two-phase loading; the eager pass runs once at session start and the lazy pass fires per file-read trigger.

1. **Eager** (`claudemd::discover_memory_files`, called from prompt build): walks `~/.coco/{CLAUDE,AGENTS}.md` then filesystem-root → CWD inclusive. In each dir loads `<dir>/.claude/CLAUDE.md`, `<dir>/{CLAUDE,AGENTS}.md`, `<dir>/{CLAUDE,AGENTS}.local.md` (case-insensitive). Each loaded file is fed through `claudemd_imports::expand_imports` so `@./other.md` and friends are recursively materialised in the same pass with cycle-break + `MAX_INCLUDE_DEPTH=5`. **Nested-worktree skip**: when CWD is a git worktree nested inside its main repo (coco agent worktrees live at `<main>/.claude/worktrees/<slug>`), `nested_worktree_roots` (via `get_git_root` + `coco_git::find_canonical_git_root`) detects the nesting and the walk skips the main repo's *checked-in* files (Project / ProjectConfig / unconditional rules) in dirs above the worktree — git already checks them out into the worktree, so loading both would duplicate the same content at distinct paths. `CLAUDE.local.md` (gitignored, main-repo-only) is still loaded. The lazy pass applies the same skip to Phase-4 cwd-level conditional rules.
2. **Lazy** (`nested_memory::traverse_for_file`, called from `app/query::QueryEngine::drain_nested_memory_triggers` at end of every turn batch): four phases per trigger file `X` —
   - **Phase 1** managed (`/etc/coco/rules`) + user (`~/.coco/rules`) **conditional** rules whose `paths:` glob matches `X`.
   - **Phase 2** `directories_to_process(X, cwd)` splits the filesystem into `nested_dirs` (CWD-exclusive → file-parent-inclusive) and `cwd_level_dirs` (root → CWD inclusive).
   - **Phase 3** for each `nested_dir`: load `{CLAUDE,AGENTS}.md`, `.claude/CLAUDE.md`, `{CLAUDE,AGENTS}.local.md`, plus `.claude/rules/**/*.md` (unconditional + matching conditional). These dirs are descendants of CWD that were *not* covered eagerly.
   - **Phase 4** for each `cwd_level_dir`: only conditional `.claude/rules/**/*.md` matching `X` (unconditional content already loaded eagerly).

Both phases share the same `expand_imports` machinery and a single `processed: HashSet<PathBuf>` of canonical paths so a file can never load twice — eagerly *or* lazily.

### Filename matching divergence

The upstream implementation only matches `CLAUDE.md` and `CLAUDE.local.md` literally. coco-rs accepts both `CLAUDE.md` *and* `AGENTS.md` (Codex / Cursor convention) at every eager and lazy load position, matched case-insensitively via `memory_filenames::find_memory_files`. `.claude/CLAUDE.md` (config-dir convention) is the one position where we keep the literal name.
