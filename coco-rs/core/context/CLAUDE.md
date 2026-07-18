# coco-context

System context assembly: environment info, memory-file discovery (`CLAUDE.md` / `AGENTS.md`), attachments, file history, plan mode, mentions, prompt building.

## Key Types

| Area (module) | Key items |
|---|---|
| Environment (`environment`) | `EnvironmentInfo`, `Platform`, `ShellKind` (cross-crate env enums owned here), `get_environment_info` |
| Memory discovery (`memory_discovery`) | `MemoryFile`, `MemoryFileSource` (`UserGlobal`/`ProjectConfig`/`Project`/`Local`), `discover_memory_files` |
| Memory filenames (`memory_filenames`) | `MEMORY_FILE_CANDIDATES` (`CLAUDE.md` + `AGENTS.md`), `find_memory_files` — case-insensitive at every position |
| `@import` expansion (`memory_imports`) | `MAX_INCLUDE_DEPTH` (5), `expand_imports`, `extract_include_paths`, `TEXT_FILE_EXTENSIONS` |
| `.claude/rules/*.md` (`memory_rules`) | `RuleFile`, `collect_rule_files`, `filter_rules_matching`, `parse_paths_field` |
| Lazy traversal (`nested_memory`) | `LoadedMemoryEntry`, `directories_to_process(file, cwd)`, `traverse_for_file` |
| Attachments (`attachment`) | `Attachment`, `AttachmentBatch`, `AttachmentBudget`, `AttachmentDeduplicator`, `collect_batched_attachments` |
| Plan mode (`plan_mode`) | `PlanModeAttachment`, `PlanWorkflow`, `Phase4Variant`, plan-file management (`get_plan` / `write_plan` / `recover_plan_for_resume` / …) |
| File tracking (`file_history`, `file_read_state`, `file_cache`) | `FileHistoryState`, `FileHistorySnapshot`, `DiffStats`; `FileReadState`, `FileReadCache` |
| Changed files (`changed_files`) | `changed_file_candidates` → permission/read-loader in `app/query` → `apply_changed_file_observations` |
| Mentions / input (`mention_resolver`, `user_input`) | `resolve_mentions`, `ProcessedInput`, `process_user_input` |
| Prompt (`prompt`, `prompt_context`) | `SystemPrompt`, `build_system_prompt`; `SystemPrompt::parts()` is the single derived view for prompt-layout metadata (`full_text()` flattens it); `prompt_context` = ordered prompt sources + deterministic epoch fingerprint for the query layer |

Also: `memory` (legacy `MemoryFileInfo` / `MemoryType`), `git_operations` / `git_utils`, `suggestions` / `prompt_suggestion`, `error` (`ContextError`). Token estimation does **not** live here — see `core/messages::token_estimation`.

Sidechat contracts (`side_chat`): `ContextualUserFragment`, `SideChatBoundaryFragment` (the read-only boundary text placed between inherited context and the first question), `BoundedContext` + `ContextFidelity` (full-prefix vs bounded-fallback), and the inheritance budgets (`MAX_TOKENS_PER_INHERITED_FRAGMENT`, `max_inherited_tokens(window)`, `MIN_RESERVED_TOKENS`). `capture_bounded_context` retains complete semantic user-turn groups and `ContextError::SideChatContextTooLarge` reports an oversized required group. See `docs/internal/sidechat-architecture.md`.

> Git worktree *creation* for agent isolation lives in `coco_coordinator::worktree` (`AgentWorktreeManager`), not here — this crate only *reads* the filesystem during memory discovery.

## Architecture

- File history uses ordered `Vec` + content-addressed files on disk (NOT HashMap). The `FileHistorySnapshot` JSON wire shape is snake_case (`message_id`, `tracked_file_backups`, `backup_file_name`, `backup_time`) and `DateTime<Utc>` for time fields (RFC 3339 strings). See `coco-session` CLAUDE.md for the cross-crate wire policy.
- Plan mode scoped by session-local `plan_slug` for fork/resume isolation.
- Phase-4 + Interview plan workflows exposed via `settings.json` (`plan_mode.phase4_variant`, `plan_mode.workflow`) — no GrowthBook / `USER_TYPE=ant` env vars. Ultraplan (CCR web UI) intentionally skipped.

## Memory-File Pipeline

Two-phase loading; the eager pass runs once at session start and the lazy pass fires per file-read trigger.

1. **Eager** (`memory_discovery::discover_memory_files`, called from prompt build): walks `~/.coco/{CLAUDE,AGENTS}.md` then filesystem-root → CWD inclusive. In each dir loads `<dir>/.claude/CLAUDE.md`, `<dir>/{CLAUDE,AGENTS}.md`, `<dir>/{CLAUDE,AGENTS}.local.md` (case-insensitive). Each loaded file is fed through `memory_imports::expand_imports` so `@./other.md` and friends are recursively materialised in the same pass with cycle-break + `MAX_INCLUDE_DEPTH=5`. **Nested-worktree skip**: when CWD is a git worktree nested inside its main repo (coco agent worktrees live at `<main>/.claude/worktrees/<slug>`), `nested_worktree_roots` (via `get_git_root` + `coco_git::find_canonical_git_root`) detects the nesting and the walk skips the main repo's *checked-in* files (Project / ProjectConfig / unconditional rules) in dirs above the worktree — git already checks them out into the worktree, so loading both would duplicate the same content at distinct paths. `CLAUDE.local.md` (gitignored, main-repo-only) is still loaded. The lazy pass applies the same skip to Phase-4 cwd-level conditional rules.
2. **Lazy** (`nested_memory::traverse_for_file`, called from `app/query::QueryEngine::drain_nested_memory_triggers` at end of every turn batch): four phases per trigger file `X` —
   - **Phase 1** managed (`/etc/coco/rules`) + user (`~/.coco/rules`) **conditional** rules whose `paths:` glob matches `X`.
   - **Phase 2** `directories_to_process(X, cwd)` splits the filesystem into `nested_dirs` (CWD-exclusive → file-parent-inclusive) and `cwd_level_dirs` (root → CWD inclusive).
   - **Phase 3** for each `nested_dir`: load `{CLAUDE,AGENTS}.md`, `.claude/CLAUDE.md`, `{CLAUDE,AGENTS}.local.md`, plus `.claude/rules/**/*.md` (unconditional + matching conditional). These dirs are descendants of CWD that were *not* covered eagerly.
   - **Phase 4** for each `cwd_level_dir`: only conditional `.claude/rules/**/*.md` matching `X` (unconditional content already loaded eagerly).

Both phases share the same `expand_imports` machinery and a single `processed: HashSet<PathBuf>` of canonical paths so a file can never load twice — eagerly *or* lazily.

### Filename matching divergence

The upstream implementation only matches `CLAUDE.md` and `CLAUDE.local.md` literally. coco-rs accepts both `CLAUDE.md` *and* `AGENTS.md` (Codex / Cursor convention) at every eager and lazy load position, matched case-insensitively via `memory_filenames::find_memory_files`. `.claude/CLAUDE.md` (config-dir convention) is the one position where we keep the literal name.
