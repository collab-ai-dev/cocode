# coco-memory

Persistent cross-session memory: per-project memory directory + 4-type
taxonomy (User / Feedback / Project / Reference) + auto-extraction +
auto-dream consolidation + per-session 9-section memory + KAIROS daily
logs + LLM-ranked recall.

## Crate Layout

```
src/
├── store/                pure data: types, frontmatter parse, MEMORY.md index + truncate
├── path/                 git-canonical resolve, validate, scope, symlink walk
├── scan.rs               single Scanner (200-cap, 30-line frontmatter read, mtime sort, manifest fmt)
├── lock.rs               PID + mtime CAS lock (auto-dream); 1h dead-PID reclaim, rollback
├── recall.rs             relevant-memory selection (LLM ranker prompt + heuristic fallback + PrefetchState)
├── compact_truncate.rs   pure session-memory section truncation
├── prompt/
│   ├── builders.rs       compose system / extract / dream / session-update prompts
│   └── text/*.md         verbatim include_str! blocks (taxonomy, what-not-to-save, …)
├── service/
│   ├── extract.rs        ExtractService — turn-end fork via AgentHandle (fork_context_messages, max_turns=5, memdir-only fence, stash + trailing run, 60s drain)
│   ├── dream.rs          DreamService — 3-gate scheduler (24h/5-session/10-min throttle), PID lock, 4-phase fork
│   └── session.rs        SessionMemoryService — 10k/5k/3 trigger gates, 9-section template, 15s wait_for_extraction, file 0o600
├── runtime.rs            MemoryRuntime + Builder; owns services, recall_state, optional SideQueryHandle for LLM ranker
├── config.rs             thin runtime adapter over `coco_config::MemoryConfig`
├── telemetry.rs          MemoryEvent enum + MemoryTelemetryEmitter trait + OtelEmitter adapter
└── lib.rs                module declarations + re-exports
```

## Key Types

| Type | Purpose |
|------|---------|
| `MemoryEntry` / `MemoryEntryType` / `MemoryFrontmatter` | parsed memory file (closed 4-type taxonomy) |
| `MemoryIndex` / `MemoryIndexEntry` | parsed `MEMORY.md` pointer list |
| `EntrypointTruncation` | line-then-byte truncation of `MEMORY.md` |
| `MemoryDir` | resolved personal + team directory pair |
| `PathValidationError` | path-validation taxonomy (null / UNC / drive-root / tilde / fullwidth / traversal) |
| `Scanner` (free fns) | `scan_memory_files`, `format_memory_manifest`, `memory_age_string`, `file_mtime_ms` |
| `PrefetchState` | per-session already-surfaced + byte-budget tracker for recall |
| `RelevantMemory` | path + truncated content + freshness header |
| `ExtractService` / `DreamService` / `SessionMemoryService` | the three async services |
| `MemoryRuntime` / `MemoryRuntimeBuilder` | session-level composer |
| `SessionEnumerator` | `Arc<dyn Fn() -> Vec<String>>` — TranscriptStore-backed lazy session lister wired by `install_session_enumerator`; consumed by `tick_dream` |
| `MemoryEvent` / `OtelEmitter` | telemetry taxonomy + OTel adapter |

## Multi-Provider Notes

- All LLM calls go through `coco-tool-runtime::SideQuery` or `AgentHandle`. **Never hardcode a model_id.**
- The recall ranker uses `SideQueryRequest::with_model_role(ModelRole::Memory)` so the operator picks provider+model in `settings.models.memory`.
- The forked extraction / dream agents inherit the parent session's `tool_overrides`, `features`, `parent_tool_filter`. The constraints layer (`AgentSpawnConstraints`) only narrows: `max_turns: 5`, `allowed_write_roots: [memdir]`.
- `MemoryConfig` (in `coco-config`) is the single source of truth for sub-toggles (extraction / team / dream / session-memory / kairos). Sub-toggles never become `Feature` variants — `Feature::AutoMemory` is the one upstream gate.

## Invariants

- `MEMORY.md` is **model-curated**; the runtime never auto-regenerates it. We only read + truncate (line 200 / 25 KB caps).
- `is_team_memory_path` uses an authoritative `MemoryDir` resolution + a `**/memory/team/**` substring fallback (gated by the secret detector).
- Path resolution is anchored to `coco_git::find_canonical_git_root` so worktrees of one repo share one memdir.
- `ExtractService::run` always sets `isolation = "fork"` + `fork_context_messages` so the child sees the parent's slice.
- The write fence resolves relative paths against `ToolUseContext::cwd_override` before checking, so a model passing `./notes.md` lands inside the fence as expected.
- `DreamService::maybe_consolidate` checks gates in order (time → scan throttle → session) and accepts `enumerate_sessions` as `FnOnce()` — the closure runs **only** after the time + scan gates pass so callers don't pay enumeration cost on every turn.
- `DreamService` rolls the lock mtime back on failure so the time-gate doesn't reset to "now"; the failure path also emits `MemoryEvent::AutoDreamFailed` for telemetry coverage.
- `MemoryRuntime::tick_dream` is the engine's per-turn entry point: it calls the installed `SessionEnumerator` (TranscriptStore-backed) lazily and threads the runtime's `transcript_dir`. Engine fans this out alongside extract + session-memory in `engine_finalize_turn`.
- `ExtractService` emits `ExtractionCoalesced` when a request stashes for a trailing run and `ExtractionError` on subagent failure — full telemetry coverage.
- `SessionMemoryService` writes the 9-section template if missing, then asks the agent to Edit-only — never overwrites the file wholesale.
- `MemoryEvent::ExtractionCompleted::files_written` sums real `Write + Edit + NotebookEdit` invocations from `AgentSpawnResponse::tool_use_counts` — no fabricated counts.

## Per-Fork canUseTool Policies (PR 4)

[`can_use_tool`](src/can_use_tool.rs) provides two policy callbacks
threaded onto every memory-fork's `AgentSpawnRequest.can_use_tool`
field. The handle runs at `coco_tool_runtime::execution::execute_tool_call`
step 3.5 BEFORE the tool's built-in `check_permissions`, so the
fork can deny / rewrite per-call without modifying the static
permission rule pipeline.

| Helper | Used by | Policy |
|---|---|---|
| `create_auto_mem_handle(memory_dir)` | `ExtractService`, `DreamService` | Allow `Read`/`Glob`/`Grep` unrestricted; Allow `Bash` IFF [`coco_shell_parser::safety::is_known_safe_command`] returns true AND command has no shell metachars (`>`, `\|`, `;`, `&`, …); Allow `Edit`/`Write` IFF `input.file_path` lexically resolves under `memory_dir`; Deny everything else |
| `create_session_mem_handle(memory_path)` | `SessionMemoryService` | Allow `Read`; Allow `Edit` IFF `input.file_path == memory_path` (exact match); Deny everything else |

The fence is **defense-in-depth**: callback (inner ring) + the
existing `AgentSpawnRequest.constraints.allowed_write_roots` field
(outer ring) both apply. Either alone would protect; both together
guard against future field-renaming drift.


## Deferred design work

Two gaps are documented but not yet implemented — both span multiple
crates and need a coordinated change set rather than a memory-crate-only
patch:

### System-prompt `cache_control` plumbing (P0-5)

**Problem**: `coco-context::SystemPrompt` carries `CacheBreakpoint`
markers in its block list, but every downstream consumer calls
`.full_text()` and discards them. The Anthropic adapter then
`collapse_text_parts` flattens any multi-part system message into one
block with `cache_control: None`. Result: any MEMORY.md edit, env-time
tick, or attachment refresh invalidates the entire system-prompt
prefix cache. The static prefix (identity + tools + CLAUDE.md) should cache
independently of dynamic content.

**Scope of fix**:
1. `coco-context::SystemPrompt` — `full_text()` becomes optional;
   add `into_parts()` returning `Vec<SystemPart { text, cache_control }>`.
2. `coco-types::LlmMessage::system(...)` — accept multi-part input.
3. `services/inference/prompt_layout` — preserve cache markers when
   materializing `AnthropicSystemBlock`.
4. `vercel-ai-anthropic/messages/convert_to_anthropic_messages` —
   stop unconditional `collapse_text_parts` when any part carries
   `cache_control`.
5. `coco-memory::MemoryRuntime::render_system_prompt_section` — split
   the truncated `MEMORY.md` body off into its own
   `SystemPart` (or push to attachment pipeline) so MEMORY.md edits
   only refresh the dynamic tail.

### Recall ranker: forced-tool → JSON schema (P1-9)

**Problem**: `MemoryRuntime::recall` uses
`SideQueryRequest::with_forced_tool` with a synthetic `select_memories`
tool. Anthropic and OpenAI honor tool-forcing reliably; Google Gemini's
function-calling shape is asymmetric. The current text-fallback path in
`runtime.rs:732-734` handles the Gemini case but pays a wasted LLM
call. `output_format: { type: 'json_schema', schema: {...} }` is
universally supported by structured-output APIs.

**Scope of fix**:
1. `coco-types::SideQueryRequest` — add
   `output_format: Option<JsonSchema>` field with new constructor
   `SideQueryRequest::with_json_schema(...)`.
2. Each `vercel-ai-{openai,anthropic,google,bytedance,openai-compatible}`
   provider — translate `JsonSchema` into the provider's structured-
   output API at request build time.
3. `services/inference/src/side_query_impl.rs` — thread `output_format`
   through.
4. `coco-memory::MemoryRuntime::recall` — switch from `with_forced_tool`
   to `with_json_schema`. Drop the synthetic tool definition; keep
   `parse_selection_response` as the response parser.

Both gaps are real and need work — but they touch cross-crate seams
that should be planned and reviewed in their own change sets rather
than bundled into a memory-crate parity pass.

## What this crate does NOT own

- The system-prompt assembly seam (`coco-context::build_system_prompt`) — memory only renders its block via `prompt::build_system_prompt_section` and hands it through.
- LLM client construction — see `coco-inference`.
- Session storage, transcripts — see `coco-session`.
- Team-memory HTTP sync — partial. `team_sync/` holds the in-tree v2 port (`service`, `watcher`, `secret_scanner`, `types`); full parity with `services/teamMemorySync/` is still pending.
- Compaction logic — `coco-compact` reads session-memory off disk via `MemoryRuntime::session_memory.current_content().await` and uses our pure `compact_truncate::truncate_session_memory_for_compact` helper.

## Cargo deps

`coco-config`, `coco-types`, `coco-tool-runtime`, `coco-frontmatter`, `coco-git`, `coco-otel`,
`coco-shell-parser` (for the auto-mem `Bash` read-only check), `async-trait`,
`tokio`, `serde`, `serde_json`, `thiserror`, `tracing`, `filetime`, `libc` (cfg(unix)).

`coco-messages` / `coco-inference` are intentionally not deps — services use the `AgentHandle`
and `SideQuery` traits from `coco-tool-runtime` instead, which keeps the layer rules clean.
