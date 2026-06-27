# coco-memory — Crate Plan

Persistent cross-session memory: per-project memory directory + 4-type
taxonomy (User / Feedback / Project / Reference) + auto-extraction +
auto-dream consolidation + per-session 9-section memory + KAIROS daily
logs + LLM-ranked recall.

## TS Source

- `memdir/{memdir,memoryTypes,paths,memoryScan,memoryAge,findRelevantMemories,teamMemPaths,teamMemPrompts}.ts`
- `services/extractMemories/{extractMemories,prompts}.ts`
- `services/SessionMemory/{sessionMemory,sessionMemoryUtils,prompts}.ts`
- `services/autoDream/{autoDream,config,consolidationLock,consolidationPrompt}.ts`
- `commands/memory/{memory.tsx,index.ts}` (slash command + dialog)
- `utils/memoryFileDetection.ts`

## Dependencies

```
coco-memory depends on:
  - coco-types (Feature gate, ModelRole)
  - coco-config (MemoryConfig — single source of truth for sub-toggles)
  - coco-tool-runtime (AgentHandleRef, SideQueryHandle)
  - coco-frontmatter, coco-git, coco-otel, coco-secret-redact
  - utils/file-watch (team-memory watcher infrastructure)

coco-memory does NOT depend on:
  - coco-messages, coco-inference (services use AgentHandle / SideQuery
    traits from coco-tool-runtime — keeps the layer rules clean)
  - coco-tools, coco-query, any app/ crate
  - coco-compact (the inverse: coco-compact reads session-memory off
    disk via `MemoryRuntime::session_memory.current_content().await`
    plus the pure helper `compact_truncate::truncate_session_memory_for_compact`)
```

## Crate Layout

```
src/
├── store/                pure data: types, frontmatter parse, MEMORY.md index + truncate
├── path/                 git-canonical resolve, validate, scope, symlink walk
├── scan.rs               single Scanner (200-cap, 30-line frontmatter read, mtime sort, manifest fmt)
├── lock.rs               PID + mtime CAS lock (auto-dream); 1h dead-PID reclaim, rollback
├── recall.rs             relevant-memory selection (LLM ranker prompt + heuristic fallback + PrefetchState)
├── compact_truncate.rs   pure session-memory section truncation helper
├── prompt/
│   ├── builders.rs       compose system / extract / dream / session-update prompts
│   └── text/*.md         verbatim include_str! blocks (taxonomy, what-not-to-save, dream, …)
├── service/
│   ├── extract.rs        ExtractService — turn-end fork via AgentHandle (max_turns=5, memdir-only fence, stash + trailing run, 60s drain)
│   ├── dream.rs          DreamService — 3-gate scheduler (24h/5-session/10-min throttle), PID lock, 4-phase fork
│   └── session.rs        SessionMemoryService — 10k/5k/3 trigger gates, 9-section template, 15s wait_for_extraction, file 0o600
├── runtime.rs            MemoryRuntime + Builder; owns services, recall_state, optional SideQueryHandle for LLM ranker
├── agent_memory.rs       per-agent persistent memory by scope (user/project/local)
├── agent_memory_snapshot.rs  baseline snapshot sync between agent dirs
├── config.rs             thin runtime adapter over `coco_config::MemoryConfig`
├── team_sync/            team-memory subdir + HTTP types (skeleton: types complete; HTTP push/pull deferred)
├── telemetry.rs          MemoryEvent enum + MemoryTelemetryEmitter trait + OtelEmitter adapter
└── lib.rs                module declarations + re-exports
```

## Key Types

| Type | Purpose |
|------|---------|
| `MemoryEntry` / `MemoryEntryType` / `MemoryFrontmatter` | parsed memory file (closed 4-type taxonomy: User / Feedback / Project / Reference) |
| `MemoryIndex` / `MemoryIndexEntry` | parsed `MEMORY.md` pointer list |
| `EntrypointTruncation` | line-then-byte truncation of `MEMORY.md` (caps 200 lines / 25KB) |
| `MemoryDir` | resolved personal + team directory pair |
| `PathValidationError` | path-validation taxonomy (null / UNC / drive-root / tilde / fullwidth / traversal) |
| Free fns in `scan.rs` | `scan_memory_files`, `format_memory_manifest`, `memory_age_string`, `file_mtime_ms` |
| `PrefetchState` | per-session already-surfaced + 60KB byte-budget tracker for recall |
| `RelevantMemory` | path + truncated content + freshness header |
| `PidLock` / `LockOutcome` | auto-dream CAS lock (PID body + mtime, 1h stale reclaim) |
| `ExtractService` | turn-end forked-extraction service (cursor + throttle + stash + trailing run) |
| `DreamService` | 3-gate consolidation scheduler |
| `SessionMemoryService` | 9-section session memory with trigger gates |
| `MemoryRuntime` / `MemoryRuntimeBuilder` | session-level composer; owns the three services + recall state + optional `SideQueryHandle` |
| `MemoryConfig` | runtime adapter over `coco_config::MemoryConfig` (field-for-field identical) |
| `MemoryEvent` / `MemoryTelemetryEmitter` / `OtelEmitter` | telemetry taxonomy + OTel adapter |

## 4-Type Memory Taxonomy

Closed enum — TS parity (`memdir/memoryTypes.ts`):

| Type | Scope | When to save |
|------|-------|--------------|
| **User** | private always | Role, preferences, knowledge — tailor behavior |
| **Feedback** | private default; team if project-wide convention | Corrections AND validated approaches; include the *why* |
| **Project** | private or team; bias toward team | Ongoing work context not derivable from code/git (deadlines, incidents, rationale). Use absolute dates |
| **Reference** | usually team | External system pointers (Linear projects, Slack channels, dashboards) |

Frontmatter format:

```markdown
---
name: <memory name>
description: <one-line — used for relevance ranking>
type: <user | feedback | project | reference>
---

<body — for feedback/project, structure as: rule/fact, then **Why:** and **How to apply:**>
```

## MemoryRuntime (the composer)

One per session. Owns the three services plus recall state and a swappable agent handle / side-query handle.

```rust
pub struct MemoryRuntime {
    pub directories: MemoryDir,
    pub config: MemoryConfig,
    pub extract: Arc<ExtractService>,
    pub dream: Arc<DreamService>,
    pub session_memory: Arc<SessionMemoryService>,
    // private:
    recall_state: Arc<PrefetchState>,
    agent_slot: AgentSlot,                              // shared swappable Arc<RwLock<AgentHandleRef>>
    side_query: tokio::sync::RwLock<Option<SideQueryHandle>>,
}

impl MemoryRuntime {
    pub async fn install_agent(&self, handle: AgentHandleRef);
    pub async fn install_side_query(&self, handle: SideQueryHandle);
    pub async fn reset(&self);                          // /clear hook
    pub fn personal_dir(&self) -> &Path;
    pub fn team_dir(&self) -> &Path;
    pub fn transcript_dir(&self) -> Option<&Path>;       // TS `getProjectDir(getOriginalCwd())`
    pub async fn render_system_prompt_section(&self) -> Option<String>;
    pub async fn recall(&self, query: &str, recent_tools: &[String]) -> Vec<RelevantMemory>;
}
```

The `agent_slot` and `side_query` cells are filled after construction via `install_*`. This decouples memory bootstrap from the engine build order: `SessionRuntime` constructs the runtime up front (so callers can call `render_system_prompt_section`), then swaps in the real `SwarmAgentHandle` and an `inference`-backed `SideQueryHandle` once those are built.

## ExtractService (turn-end forked extraction)

TS source: `services/extractMemories/extractMemories.ts`.

After every eligible turn, spawn a forked subagent with a 5-turn cap and a memdir-only write fence. The agent reads existing memories (manifest pre-injected into its prompt) then writes/edits memory files based on the conversation slice since the last cursor.

Gate sequence (in order):

1. `config.extraction_enabled` — TS `tengu_passport_quail`
2. `coco_types::Feature::AutoMemory` enabled — TS `isAutoMemoryEnabled()`
3. Coalesce: if a fork is in flight, stash this trigger as `pending_trailing` and exit
4. Mutual exclusion: skip if main agent already wrote to memory since the cursor — **also advances the cursor past this turn** so the next eligible turn doesn't re-consider these messages (TS `extractMemories.ts:347-360`)
5. Throttle: `state.turns_since_last >= config.extraction_throttle` (default 1)
6. Cursor advance on success; on failure the cursor stays so the next attempt retries the same range

Key constants:

| Constant | Value | TS reference |
|---------|-------|--------------|
| `DEFAULT_DRAIN_TIMEOUT` | 60s | `drainPendingExtraction(60_000)` |
| `extraction_max_turns` (config default) | 5 | `runForkedAgent({...maxTurns: 5})` |
| `extraction_throttle` (config default) | 1 | `tengu_bramble_lintel` |

Telemetry: `MemoryEvent::ExtractionCompleted | ExtractionSkippedDirectWrite | ExtractionToolDenied` map to TS `tengu_extract_memories_extraction / _skipped_direct_write / tengu_auto_mem_tool_denied`.

The forked agent's tool fence is enforced by `AgentSpawnConstraints::allowed_write_roots = [memory_dir]`, mirroring TS `createAutoMemCanUseTool` (Read/Grep/Glob unrestricted; read-only Bash; Write/Edit/NotebookEdit only inside the memdir).

`drain` (called from `app/cli/main.rs::run_sdk_mode`, `app/cli/tui_runner::drain_pending_memory_extraction`, and `app/cli/headless::run_chat_with_options`) waits up to 60s for an in-flight fork before letting the process exit, so partial writes don't get cut off.

## DreamService (auto-dream consolidation)

TS source: `services/autoDream/`.

Background memory consolidation. Fires a forked subagent when the three gates plus the lock all pass. The dream agent merges related entries, resolves contradictions, and prunes stale `MEMORY.md` pointers.

Gate sequence:

1. **Time gate**: `hours_since(last_consolidation) >= dream_min_hours` (default 24)
2. **Scan throttle**: at most one full session-set scan per `SCAN_THROTTLE = 10min` (`SESSION_SCAN_INTERVAL_MS` in TS)
3. **Session gate**: at least `dream_min_sessions` (default 5) other sessions touched the project since the last consolidation
4. **Lock acquire**: `lock.rs::try_acquire_lock` — PID body + mtime; stale at 60min OR dead-PID reclaim. Returns `prior_mtime` for rollback.

The 4-phase consolidation prompt (Orient / Gather / Consolidate / Prune) is verbatim from TS `consolidationPrompt.ts`; see `prompt/text/dream.md`. `{MEMORY_ROOT}` and `{TRANSCRIPT_DIR}` placeholders are substituted at build time.

On failure, `lock::rollback_lock_mtime` rewinds to the previous mtime (or unlinks when prior was 0) so the time gate doesn't reset to "now". KAIROS mode disables auto-dream (the daily-log paradigm doesn't compose with merge-style consolidation).

**Manual `/dream`** uses [`DreamService::force`] which bypasses the time / session / scan-throttle gates but **still acquires the PID + mtime CAS lock**, so a manual run cannot race with an in-flight auto-dream. It also still respects `dream_enabled` / `kairos_mode` (TS parity: those settings turn the entire feature off, manual or not). Wired from `tui_runner::run_dream_consolidation` and `sdk_runner` `/dream` short-circuit.

Telemetry: `MemoryEvent::AutoDreamFired | AutoDreamCompleted` map to `tengu_auto_dream_fired / _completed`.

## SessionMemoryService (9-section per-session memory)

TS source: `services/SessionMemory/` (~600 LOC).

Persists a structured 9-section summary of the live conversation to disk so context can be restored after compaction or `--resume`.

Trigger gate (both must hold):

1. **Token gate**: `current_tokens - last_extraction_tokens >= session_memory_update_tokens` (default 5_000), or
   `current_tokens >= session_memory_init_tokens` (default 10_000) for the first extraction.
2. **Activity gate**: either
   - tool calls in the last assistant turn `>= session_memory_tool_calls` (default 3), or
   - natural break (zero tool calls in the last turn).

Manual override: `force(tokens)` (bound to `/summary`).

Storage: `<config_home>/session-memory/<session_id>.md`, file mode `0o600`, dir mode `0o700`.

9-section template (verbatim from `prompt/text/session_template.md`):

1. Session Title
2. Current State
3. Task Specification
4. Files and Functions
5. Workflow
6. Errors & Corrections
7. Codebase Documentation
8. Learnings
9. Key Results & Worklog

Per-section budget: `session_memory_per_section_tokens` (2_000) ; total `session_memory_total_tokens` (12_000).

`wait_for_extraction(timeout)` (default 15s) is awaited by the compaction path so a still-running extraction doesn't get clobbered.

`compact_truncate::truncate_session_memory_for_compact` is a pure helper that the compact crate uses to fit the saved session memory inside the post-compact budget.

## Path resolution + security

Memory directories are anchored to the project's git root via `coco_git::find_canonical_git_root` so all worktrees of one repo share one memdir. The team directory is `<personal>/team/` (TS parity).

Override precedence (first match wins):

1. `COCO_MEMORY_PATH_OVERRIDE` env (operator)
2. `COCO_REMOTE_MEMORY_DIR` env (swarm leader → teammate propagation)
3. `settings.memory.directory`
4. Default: `<config_home>/projects/<sanitized-cwd>/memory/`

Validation rejects: null bytes, UNC paths, drive-root, unexpanded tilde, full-width unicode traversals, URL-encoded `../`, backslash absolutes. `is_within_memory_dir` plus a `realpath_deepest_existing` symlink walk guard the write fence.

Sub-toggles + corresponding env force-disables:

| Toggle | Env force-off |
|--------|---------------|
| `extraction_enabled` | `COCO_MEMORY_EXTRACTION_DISABLE` |
| `dream_enabled` | `COCO_MEMORY_DREAM_DISABLE` |
| `session_memory_enabled` | `COCO_MEMORY_SESSION_MEMORY_DISABLE` |
| `kairos_mode` | `COCO_MEMORY_KAIROS` (truthy = enable) |

The upstream gate is `coco_types::Feature::AutoMemory`. Sub-toggles are *not* `Feature` variants — they live flat on `MemoryConfig` per project convention.

## Recall (LLM-ranked relevant-memory selection)

TS source: `memdir/findRelevantMemories.ts`, `memdir/memoryScan.ts`.

`MemoryRuntime::recall(query, recent_tools)`:

1. Cold-start short-circuit: if `MEMORY.md` doesn't exist, return empty.
2. `scan_memory_files(personal_dir)` — capped at 200 files, frontmatter-only first 30 lines, mtime descending.
3. With a `SideQueryHandle` plugged in: `SideQueryRequest::with_forced_tool` against `ModelRole::Memory` to coerce a JSON `{selected_memories: string[5]}` response. TS `tool_choice: { type: "tool", name: "select_memories" }`.
4. Without a handle: `select_heuristic` falls back to recency.
5. `PrefetchState` enforces per-session already-surfaced dedup + a 60KB byte budget so memories don't snowball over a long session.

The ranker honors `recent_tools` so usage-reference docs get deprioritized while the agent is actively driving those tools.

## System-prompt block

`MemoryRuntime::render_system_prompt_section()` returns the `# auto memory` block injected by `coco_context::build_system_prompt`. Three variants:

| Variant | Trigger |
|---------|---------|
| **Auto** | `auto_memory` enabled, team off, KAIROS off |
| **Combined** | `team_memory_enabled` true |
| **Kairos** | `kairos_mode` true (assistant daily-log paradigm) |

The block reads the truncated `MEMORY.md` (and team `MEMORY.md` for Combined) and concatenates verbatim taxonomy + how-to-save + when-to-access blocks (`prompt/text/*.md`).

Optional sections:

- `searching_past_context_enabled` (TS `tengu_coral_fern`): inserts a `## Searching past context` block with grep examples for the memory directory. Off by default.
- `extra_guidelines`: arbitrary policy text from operator (e.g. `COCO_COWORK_MEMORY_EXTRA_GUIDELINES`).

`skip_index` (TS `tengu_moth_copse`): when set, the model is told to write topic files only — skips the two-step indexing instruction.

## Per-agent memory + snapshots

`agent_memory.rs` and `agent_memory_snapshot.rs` give each subagent its own `MEMORY.md` namespace partitioned by `MemoryScope::{User, Project, Local}`. The snapshot module syncs a baseline between scopes (e.g. promote a project memory to user scope on demand). Loaded into the subagent's system prompt at spawn (`coordinator/agent_handle/spawn.rs`).

## Team memory

`team_sync/` (~880 LOC) implements the HTTP-backed team-memory sync:

- Types: `TeamMemoryContent`, `TeamMemoryData`, `SyncState`, `TeamMemoryHashesResult`, `TeamMemorySyncFetchResult`, `TeamMemorySyncPushResult`, `PushEntry`, `SkippedSecretFile`
- `service::pull(state, base_url, repo_slug, bearer, etag)` — `GET /api/...` with `If-None-Match`, 304 short-circuit, 404 = empty, updates `state.last_known_checksum` + `state.server_checksums`
- `service::push(state, entries)` — delta upload (only entries whose `sha256:<hex>` doesn't match `state.server_checksums`), batched under `MAX_PUT_BODY_BYTES = 200_000`
- `service::apply_pulled_content(dir, content)` — write server response to local team dir
- `service::compute_content_hash` — `sha256:<hex>` matching the server format
- `secret_scanner::scan_for_secrets` — filter sensitive files before push (delegates to `coco_secret_redact`)
- `watcher::spawn_watcher(WatcherConfig)` — file-watch driven push trigger
- Constants matching TS: `MAX_FILE_SIZE_BYTES = 250_000`, `MAX_PUT_BODY_BYTES = 200_000`, `SYNC_TIMEOUT_MS`

**Wiring status**: implemented but not yet driven from `app/cli`. `team_memory_enabled` flips on the system-prompt Combined variant today; the watcher + auth-token plumbing into the session lifecycle is the remaining integration step. Path validation against URL-encoded / fullwidth traversals is the security check that should land alongside the wire-up.

## Configuration (`coco_config::MemoryConfig`)

Single source of truth — `MemoryConfig` in coco-memory is a field-for-field adapter.

| Field | Default | Notes |
|-------|---------|-------|
| `directory` | `None` | Override: env or `settings.memory.directory` |
| `skip_index` | `false` | TS `tengu_moth_copse` |
| `kairos_mode` | `false` | Daily-log assistant mode |
| `extraction_enabled` | `true` | TS `tengu_passport_quail` |
| `extraction_throttle` | `1` | TS `tengu_bramble_lintel` (every Nth turn) |
| `extraction_max_turns` | `5` | Forked-agent cap |
| `team_memory_enabled` | `false` | TS `tengu_herring_clock` |
| `dream_enabled` | `true` | Auto-dream consolidation |
| `dream_min_hours` | `24` | TS `tengu_onyx_plover.minHours` |
| `dream_min_sessions` | `5` | TS `tengu_onyx_plover.minSessions` |
| `session_memory_enabled` | `true` | 9-section per-session memory |
| `session_memory_init_tokens` | `10_000` | Floor before first extraction |
| `session_memory_update_tokens` | `5_000` | Token-growth gate |
| `session_memory_tool_calls` | `3` | Activity gate |
| `session_memory_per_section_tokens` | `2_000` | Per-section cap |
| `session_memory_total_tokens` | `12_000` | Aggregate cap |
| `searching_past_context_enabled` | `false` | TS `tengu_coral_fern` |

## Telemetry

```rust
pub enum MemoryEvent {
    MemdirLoaded { line_count, byte_count, was_truncated, was_byte_truncated, has_team },
    MemdirDisabled { reason: DisableReason },                  // EnvVar / Settings / BareMode / RemoteMode / FeatureGate
    ExtractionToolDenied { tool_name },
    ExtractionSkippedDirectWrite { message_count },
    ExtractionCompleted { turn_count, input_tokens, output_tokens, files_written, duration_ms },
    AutoDreamFired { hours_since_last, sessions_since_last },
    AutoDreamCompleted { sessions_reviewed, files_changed, duration_ms },
    SessionMemoryExtracted { input_tokens, output_tokens, duration_ms },
}
```

Each variant maps to a TS `tengu_*` event (`tengu_memdir_loaded`, `tengu_memdir_disabled`, `tengu_auto_mem_tool_denied`, `tengu_extract_memories_skipped_direct_write`, `tengu_extract_memories_extraction`, `tengu_auto_dream_fired`, `tengu_auto_dream_completed`, `tengu_session_memory_extraction`). The trait `MemoryTelemetryEmitter` lets the SDK / TUI / headless callers route events into OTel via `OtelEmitter` (default) or a bespoke sink.

## Integration sites (where the runtime is wired)

| Site | What | When |
|------|------|------|
| `app/cli/src/session_runtime.rs::SessionRuntime::new` | Build `MemoryRuntime` (gated on `Feature::AutoMemory`); fire-and-forget dream gate-check; install agent + side-query | Session bootstrap |
| `app/cli/src/session_runtime.rs::SessionRuntime::clear_conversation` | `runtime.reset().await` — clears recall state, extract cursor, session-memory init flag | `/clear` (full Conversation scope only) |
| `app/query/src/engine_finalize_turn.rs::finalize_turn_post_tools` | `session_memory.maybe_extract` + `extract.maybe_extract` | Every turn end |
| `coco-context::build_system_prompt` | Reads `runtime.render_system_prompt_section()` | Once per session, threaded through |
| `app/cli/src/sdk_server/sdk_runner.rs` | `/dream` and `/summary` short-circuit paths call `dream.maybe_consolidate` and `session_memory.force` | Slash-command dispatch |
| `app/cli/src/tui_runner.rs::drain_pending_memory_extraction` | `extract.drain(60s)` before `SessionEnded` | TUI shutdown |
| `app/cli/src/main.rs::run_sdk_mode` | `extract.drain(60s)` after dispatch loop exits | SDK shutdown |
| `app/cli/src/headless.rs::run_chat_with_options` | `extract.drain(60s)` after the single-shot turn | Headless one-shot |
| `coordinator/src/agent_handle/spawn.rs` | Loads per-agent memory block via `agent_memory::load_agent_memory_prompt` | Subagent spawn |
| `commands/src/handlers/memory_dialog.rs` + `app/tui/src/update/overlay.rs::open_memory_entry_async` | `/memory` file picker → write empty file (idempotent) → spawn `$VISUAL || $EDITOR` | `/memory` slash command |
| `commands/src/handlers/dream.rs` (sentinel) → `app/cli/src/tui_runner.rs::run_dream_consolidation` | Manual dream trigger | `/dream` slash command |

## Invariants

- `MEMORY.md` is **model-curated**; the runtime never auto-regenerates it. It is read + truncated only.
- `is_team_memory_path` uses an authoritative `MemoryDir` resolution + a `**/memory/team/**` substring fallback (gated by the secret detector).
- Path resolution is anchored to `coco_git::find_canonical_git_root` so worktrees of one repo share one memdir.
- `ExtractService::run` always sets `isolation = "fork"` + `fork_context_messages` so the child sees the parent's slice — TS parity (`AgentTool.tsx:622-632`).
- The write fence resolves relative paths against `ToolUseContext::cwd_override` before checking, so a model passing `./notes.md` lands inside the fence.
- `DreamService` rolls the lock mtime back on failure so the time gate doesn't reset to "now".
- `SessionMemoryService` writes the 9-section template if missing, then asks the agent to Edit-only — never overwrites the file wholesale.
- `MemoryEvent::ExtractionCompleted::files_written` sums real `Write + Edit + NotebookEdit` invocations from `AgentSpawnResponse::tool_use_counts` — no fabricated counts.

## What this crate does NOT own

- The system-prompt assembly seam (`coco-context::build_system_prompt`) — memory only renders its block via `prompt::build_system_prompt_section` and hands it through.
- LLM client construction — see `coco-inference`.
- Session storage, transcripts — see `coco-session`.
- Team-memory wire-up from `app/cli` — the HTTP `pull`/`push` and the file-watch `spawn_watcher` live in `team_sync/`, but driving them from the session lifecycle (auth-token plumbing, watcher start/stop, push retry policy) is not in this crate.
- Compaction logic — `coco-compact` reads session-memory off disk via `MemoryRuntime::session_memory.current_content().await` and uses our pure `compact_truncate::truncate_session_memory_for_compact` helper.

## Deferred / not ported

- **Team-memory wire-up from `app/cli`** — `team_sync::{pull, push, spawn_watcher}` are implemented; no caller drives them yet (no auth-token plumbing, no session-lifecycle integration). `team_memory_enabled` only affects the system-prompt Combined variant today.
- **`tengu_memory_recall_shape` telemetry** — TS `findRelevantMemories.ts:66-72`. Optional analytics on selection-rate. Skipped to keep telemetry surface minimal; reintroduce if recall quality needs measurement.
- **TS `#` prefix shortcut for memory-add** — only orphaned renderers (`UserMemoryInputMessage.tsx`, `MemoryUpdateNotification.tsx`) remain in current TS. Producer side was removed upstream; not porting is correct parity.
- **TS `USER_TYPE='ant'` gates** — intentionally dropped per `CLAUDE.md` (Anthropic-internal visibility).

## Multi-Provider Notes

- All LLM calls go through `coco-tool-runtime::SideQuery` or `AgentHandle`. **Never hardcode a model_id.**
- The recall ranker uses `SideQueryRequest::with_model_role(ModelRole::Memory)` so the operator picks provider+model in `settings.models.memory`.
- The forked extraction / dream agents inherit the parent session's `tool_overrides`, `features`, `parent_tool_filter`. The constraints layer (`AgentSpawnConstraints`) only narrows: `max_turns: 5`, `allowed_write_roots: [memdir]`.
- `MemoryConfig` (in `coco-config`) is the single source of truth for sub-toggles (extraction / team / dream / session-memory / kairos / searching-past-context). Sub-toggles never become `Feature` variants — `Feature::AutoMemory` is the one upstream gate.
