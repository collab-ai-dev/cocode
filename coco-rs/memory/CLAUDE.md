# coco-memory

Persistent cross-session memory: per-project memory directory + 4-type
taxonomy (User / Feedback / Project / Reference) + auto-extraction +
auto-dream consolidation + per-session 9-section memory + KAIROS daily
logs + LLM-ranked recall.

## Crate Layout

```
src/
├── store/                   pure data: types, frontmatter parse, MEMORY.md index + truncate (no I/O)
├── path/                    git-canonical resolve, validate, scope, symlink walk
├── scan.rs                  single Scanner (200-cap, 30-line frontmatter read, mtime sort, manifest fmt)
├── lock.rs                  PID + mtime CAS lock (auto-dream); 1h dead-PID reclaim, rollback
├── recall.rs                selection prompt/parse helpers + PrefetchState (ranker call lives in runtime)
├── compact_truncate.rs      pure session-memory section truncation
├── prompt/builders.rs       system / extract / dream / session-update prompt builders
├── service/
│   ├── extract.rs           ExtractService — turn-end fork via AgentHandle (fork_context_messages, max_turns=5, memdir-only fence, stash + trailing run, 60s drain)
│   ├── dream.rs             DreamService — 3-gate scheduler (24h/5-session/10-min throttle), PID lock, 4-phase fork
│   └── session.rs           SessionMemoryService — 10k/5k/3 trigger gates, 9-section template, 15s wait_for_extraction, file 0o600
├── runtime.rs               MemoryRuntime + Builder; owns services, recall_state, optional SideQueryHandle
├── config.rs                thin runtime adapter over `coco_config::MemoryConfig`
├── can_use_tool.rs          per-fork canUseTool policy callbacks (see table below)
├── journal.rs               best-effort host-side appends to `memory-journal.jsonl` — sibling of the memdir, OUTSIDE both fence rings so forks can't touch it; merged with the skill journal by `/journey`
├── mutate.rs                user-initiated mutations (`/journey` delete); a user action, not autonomous regeneration — deliberate carve-out from the MEMORY.md invariant
├── notice.rs                MemoryUserNotice + NoticeInbox; engine drains via `MemoryRuntime::drain_user_notices` → "Saved N memories" transcript line (separate from telemetry: needs exactly-once user-visible delivery)
├── agent_memory.rs          per-agent persistent memory `.../agent-memory/<agentType>/MEMORY.md`, scope from agent frontmatter `memory: user|project|local`; user scope follows COCO_CONFIG_HOME
├── agent_memory_snapshot.rs project-shipped baseline snapshots (`agent-memory-snapshots/<agentType>/` + snapshot.json) synced at bootstrap via `.snapshot-synced.json`
├── kairos/                  KAIROS daily logs (daily_log + rollover)
├── team_sync/               in-tree team-memory sync v2 port (service, watcher, secret_scanner, types)
├── telemetry.rs             MemoryEvent enum + MemoryTelemetryEmitter trait + OtelEmitter adapter
└── lib.rs                   module declarations + re-exports
```

## Key Types

| Type | Purpose |
|------|---------|
| `MemoryEntry` / `MemoryEntryType` / `MemoryFrontmatter` | parsed memory file (closed 4-type taxonomy) |
| `MemoryIndex` / `MemoryIndexEntry` / `EntrypointTruncation` | parsed `MEMORY.md` pointer list + line-then-byte truncation |
| `MemoryDir` | resolved personal + team directory pair |
| `PathValidationError` | path-validation taxonomy (null / UNC / drive-root / tilde / fullwidth / traversal) |
| `PrefetchState` / `RelevantMemory` | per-session surfaced+budget tracker; recall result (path + truncated content + freshness) |
| `ExtractService` / `DreamService` / `SessionMemoryService` | the three async services |
| `MemoryRuntime` / `MemoryRuntimeBuilder` | session-level composer |
| `SessionEnumerator` | `Arc<dyn Fn() -> Vec<String>>` — lazy TranscriptStore-backed session lister for auto-dream scheduling |
| `MemoryUserNotice` / `NoticeInbox` | user-visible "Saved/Improved N memories" channel |
| `MemoryEvent` / `OtelEmitter` | telemetry taxonomy + OTel adapter |

## Multi-Provider Notes

- All LLM calls go through `coco-tool-runtime::SideQuery` or `AgentHandle`. **Never hardcode a model_id.**
- The recall ranker rides `ModelRole::Memory` so the operator picks provider+model in `settings.models.memory`. `MemoryRuntime::recall` prefers each provider's native structured-output API (`SideQueryRequest::with_json_schema`) and falls back to the forced-tool synthetic `select_memories` path (`extract_recall_selection` / `parse_selection_response`) on malformed or failed responses.
- Forked extraction / dream agents inherit the parent's `tool_overrides`, `features`, `parent_tool_filter`; prompt-visible write/edit tool names derive from a runtime-only `Arc<ToolOverrides>` — never branch prompt text on model names. `AgentSpawnConstraints` only narrows: `max_turns: 5`, `allowed_write_roots: [memdir]`.
- `MemoryConfig` (in `coco-config`) is the single source of truth for sub-toggles (extraction / team / dream / session-memory / kairos). Sub-toggles never become `Feature` variants — `Feature::AutoMemory` is the one upstream gate.

## Invariants

- `MEMORY.md` is **model-curated**; the runtime never auto-regenerates it — only read + truncate (200-line / 25 KB caps). User-initiated prunes via `mutate.rs` are the documented exception.
- `is_team_memory_path` uses authoritative `MemoryDir` resolution + a `**/memory/team/**` substring fallback (gated by the secret detector).
- Path resolution is anchored to `coco_git::find_canonical_git_root` so worktrees of one repo share one memdir.
- `ExtractService::run` always sets `isolation = "fork"` + `fork_context_messages` so the child sees the parent's slice. It emits `ExtractionCoalesced` on stash-for-trailing-run and `ExtractionError` on subagent failure.
- The write fence resolves relative paths against `ToolUseContext::cwd_override` before checking, so `./notes.md` lands inside the fence as expected.
- `DreamService::maybe_consolidate` checks gates in order (time → scan throttle → session); `enumerate_sessions` is `FnOnce` and runs only after the first two gates pass. It intentionally does NOT mirror CC's server-backed `teamMemoryServerStatus` gate — personal-only background dream is supported.
- Dream scheduling is anti-spin: the scan throttle is stamped before session enumeration and stays stamped on session-gate/lock/fork failure; only an acquired lock is rolled back. Failures emit `AutoDreamFailed { phase: Fork, error_class }` (class prefix, not full error).
- `AutoDreamSkipped` fires only for the upstream-instrumented branches: `reason: sessions` (with counts) and `reason: lock`; time gate / throttle / disabled / KAIROS skips stay silent (CC 2.1.193 parity).
- `build_dream_prompt` mirrors CC 2.1.193: `logs/YYYY/MM/DD/<id>-<title>.md` session logs, Phase-3 CLAUDE.md reconciliation, `team/` pruning guidance only when team recall is enabled.
- `AutoDreamFired`/`AutoDreamCompleted` carry `team_memory_enabled` (via `is_team_recall_enabled()`); Completed reports `files_touched_count` from `AgentSpawnResponse::paths_written` only and best-effort `daily_logs_found` (missing logs → 0).
- `MemoryRuntime::finalize_turn` is the engine's per-turn entry point: schedules extraction, session-memory, and auto-dream (lazy `SessionEnumerator`, threads `transcript_dir`); its `FinalizeTurnReport` carries `index_warnings` + drained user notices.
- After main-agent file mutations (`Edit`/`Write`/`NotebookEdit`, or `apply_patch` for patch-based models), `finalize_turn` checks edited memory indexes against the CC 80%-warning / 70%-compact-target rule (local `MEMORY.md`: 200-line + 25 KB caps; mounted team `promptIndex`: `promptIndexMaxBytes ?? 25 KB`) and returns warnings as model-visible `<system-reminder>` attachments.
- Cowork guidelines gate: `COCO_COWORK_MEMORY_GUIDELINES` (mirrors CC's `CLAUDE_COWORK_MEMORY_GUIDELINES`) is folded by `coco-config` into `MemoryConfig::guidelines`; when non-empty, `render_system_prompt_section` returns only `# auto memory\n{truncated}` and skips bundled taxonomy, root `MEMORY.md`, mounted prompt indexes, and `extra_guidelines`.
- Mounted stores follow CC's `w$t` routing: non-empty `memory_stores` with no writable `scope:"user"` store → team-only prompt shape; no private memdir exposed, mounted `promptIndex` content is the loaded index surface. Every configured `promptIndex` fetch emits `MemoryEvent::MemoryPromptIndex` (missing file = Ok/empty; unreadable = Error and omitted).
- `SessionMemoryService` writes the 9-section template if missing, then asks the agent to Edit-only — never overwrites the file wholesale.
- `ExtractionCompleted::files_written` prefers real paths from `AgentSpawnResponse::paths_written` (incl. `apply_patch`), falling back to native write-tool counts only for legacy drivers — no fabricated counts.
- System-prompt caching: cache markers now flow as parts — `coco-context::SystemPrompt::parts()` (`CacheHint::Breakpoint`) → `services/inference::prompt_layout` (`put_system_prompt_parts`) → provider `cache_control`; memory's section rides that pipeline rather than a monolithic `full_text()`.

## Per-Fork canUseTool Policies

[`can_use_tool`](src/can_use_tool.rs) provides policy callbacks threaded onto
every memory-fork's `AgentSpawnRequest.permissions.can_use_tool`. The handle
runs in `coco_tool_runtime::execution::execute_tool_call` BEFORE the tool's
built-in `check_permissions`, so the fork can deny / rewrite per-call without
touching the static permission pipeline. `*_with_telemetry` variants
(`create_auto_mem_handle_with_telemetry`, `create_auto_dream_handle_with_telemetry`)
wrap the same policies with `MemoryEvent` emission.

| Helper | Used by | Policy |
|---|---|---|
| `create_auto_mem_handle(memory_dir)` | `ExtractService` | Allow `Read`/`Glob`/`Grep` unrestricted; `Bash` IFF `coco_shell_parser::safety::is_known_safe_command` AND no shell metachars; `Edit`/`Write` IFF `file_path` resolves (against the canUseTool cwd) to a `.md` under `memory_dir`; `apply_patch` when every affected path does; deny everything else |
| `create_auto_dream_handle(memory_dir)` | `DreamService` | Same as extract, plus simple `rm` of absolute `.md` paths under `memory_dir` (no recursive flags, globs, redirects, pipelines, outside paths) — CC 2.1.193 auto-dream pruning |
| `create_session_mem_handle(memory_path)` | `SessionMemoryService` | Allow `Read`; `Edit` IFF `file_path == memory_path` exactly; deny everything else |

The fence is **defense-in-depth**: callback (inner ring) +
`constraints.allowed_write_roots` (outer ring) both apply — either alone would
protect; both together guard against field-renaming drift.

## What this crate does NOT own

- The system-prompt assembly seam (`coco-context::build_system_prompt`) — memory only renders its block via `prompt::build_system_prompt_section` and hands it through.
- LLM client construction — see `coco-inference`.
- Session storage, transcripts — see `coco-session`.
- Team-memory HTTP sync — partial: `team_sync/` holds the in-tree v2 port; full parity with upstream `services/teamMemorySync/` is still pending.
- Compaction logic — `coco-compact` reads session-memory off disk via `MemoryRuntime::session_memory.current_content().await` and uses our pure `compact_truncate::truncate_session_memory_for_compact`.

`coco-messages` / `coco-inference` are intentionally not deps — services use the
`AgentHandle` and `SideQuery` traits from `coco-tool-runtime` instead, keeping
the layer rules clean.
