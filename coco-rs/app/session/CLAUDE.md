# coco-session

JSONL-canonical session persistence, transcript history, cost recovery, title
generation, and per-process concurrent-session registry. No `<session-id>.json`
sidecar — every session-level fact (title, tags, model, created/updated_at,
message counts) is derived from the transcript's first entry plus trailing
metadata. Transcript-as-truth removes state-drift between sidecar and
transcript. Wire **field names** are Rust-idiomatic snake_case rather than
camelCase.

## Wire-format policy

**Field names on disk are snake_case; wire semantics are byte-stable.** Every fact a session carries (chain UUIDs, timestamps, tool_use_ids, file-history snapshot chain, content-replacement records, marble-origami staged ranges) is preserved with well-defined semantics. Specifics:

- Field names on disk are **snake_case** (`parent_uuid`, `session_id`,
  `is_sidechain`, `tool_use_id`, `message_id`, …). No `serde(rename_all =
  "camelCase")` on session/file-history wire types. Adding a new struct field
  picks up the Rust-natural name automatically.
- Enum discriminator tags are consistent: `MetadataEntry` uses `type:`
  (kebab-case values for the semantic taxonomy — `custom-title`,
  `file-history-snapshot`, …); `SystemMessage` and other tagged enums use
  `kind:` matching the rest of coco-rs.
- External JSONL formats are **not** read directly. One-time migration goes through `coco_session::import_ts` (TODO). Cross-implementation runtime interop is **not** a goal.
- Inner `message.content` blocks keep their Anthropic API field names
  (`tool_use_id`, `tool_name`, `is_error`, …) because those ARE the wire
  format we pass to/from the LLM. This boundary is independent of the
  envelope serde.

The local Event Hub (`coco-hub-server::local_store`) reads through
`SessionCatalog`/`SessionStore` and projects session entries into Hub rows.
The disk-backed path still sees snake_case `TranscriptEntry`/`MetadataEntry`
wire shapes; cross-language hub clients (the embedded web UI) continue to
receive camelCase via the `hub/server/src/store/mod.rs` HTTP DTOs — that
boundary is separate from disk wire.

## Key Types

| Type | Purpose |
|------|---------|
| `Session` | Derived view `{id, created_at, updated_at?, model, working_dir, title?, message_count, total_tokens, tags}` — built from `TranscriptMetadata`, never persisted as its own file |
| `SessionManager` | `create` (in-memory only) / `load` / `resume` / `list` / `delete` / `most_recent` / `cleanup(keep_count)` / `cleanup_older_than` |
| `TranscriptStore`, `TranscriptEntry`, `TranscriptMetadata`, `TranscriptUsage` | Append-only JSONL transcript with per-entry usage. Path layout via `Arc<ProjectPaths>` |
| `Entry`, `MetadataEntry` | Tagged union: transcript message vs metadata entry (custom-title, tag, last-prompt, summary, file-history-snapshot, marble-origami-{commit,snapshot}, content-replacement, session-seq-watermark, …). `SessionSeqWatermark` records the durable `session_seq` high-water mark (folded as `TranscriptMetadata.session_seq_watermark`) so a restarted process skip-aheads its seq counter on resume. |
| `ModelCostEntry` | Per-model cost row inside a `CostSummary` metadata entry. Resume-side cost replay is not yet wired (`coco-messages::CostTracker::start_with_recovery` consumes the in-memory tracker only); the typed entry stays so write-path emission keeps a stable shape. |
| `PromptHistory`, `HistoryEntry` | Ring of user-typed prompts (for up-arrow recall) |
| `AgentMetadata` | Sidecar for AgentTool spawns at `<sid>/subagents/agent-<id>.meta.json` |
| `recovery::*` | Crash recovery — partial transcript repair + last-good-state detection |
| `storage::*` | Low-level JSON / JSONL IO. `storage_chain` owns agent-parent chain selection/parsing; `storage_metadata` owns lite metadata scans, path resolution, and cross-project enumeration. |
| `title_generator::*` | Auto-titling via `ModelRole::Fast` (short session label after first turn) |
| `SessionRegistry`, `SessionRegistration`, `SessionKind`, `SessionStatus` | PID-file registry for `coco ps` — drop the guard to deregister, write-lock-serialized live patches |
| `count_concurrent_sessions`, `is_bg_session`, `read_session_registration` | Cross-process enumeration helpers |

## Layout

All session artifacts live under `<memory_base>/projects/<slug>/` (resolved via
`coco_paths::ProjectPaths`). `memory_base` defaults to
`coco_config::config_home()` and is overridable via `COCO_REMOTE_MEMORY_DIR`
(CCR / swarm leader). The slug is the `[a-zA-Z0-9]→-` sanitized + NFC-
normalised session cwd / worktree path, with a djb2 suffix for paths over
200 bytes — see `coco-paths::ProjectSlug`.

```
<memory_base>/
├── projects/
│   └── <slug>/                              # per-project root
│       ├── <session-id>.jsonl               # append-only transcript
│       └── <session-id>/                    # per-session artifacts
│           ├── subagents/
│           │   ├── agent-<id>.jsonl         # bg agent transcript
│           │   └── agent-<id>.meta.json     # AgentMetadata sidecar
│           ├── remote-agents/
│           │   └── remote-agent-<tid>.meta.json
│           ├── tool-results/                # persisted tool-result blobs
│           └── session-memory/
│               └── summary.md               # 9-section per-session memory
└── sessions/                                # cross-PID registry
    ├── <pid>.json                           # SessionRegistration (claude ps)
    └── ...
```

`history.json` (PromptHistory) lives under `<config_home>` directly, not in
`<memory_base>/projects/...` — it's user-typed input recall, not session
state.

## Worktree path invariant

Session transcripts are keyed by the exact session cwd / worktree path. Do not collapse transcript paths through
`coco_git::find_canonical_git_root`. The memory subsystem owns its own
canonical-git-root path resolution so linked worktrees can share memories while
keeping transcripts separate.

## Concurrent session registry

`SessionRegistry` writes one `<pid>.json` file per top-level session under
`<config_home>/sessions/`. Subagents (those with a non-null `agent_id`) intentionally
do NOT register — counting them would conflate swarm activity with real
concurrency. Live patches (`update_session_name`, `update_session_bridge_id`,
`update_session_activity`) are serialised via the registry's internal write
lock so a teleport rename racing with the per-turn activity update can't lose
fields. The stale-PID sweep in `count_concurrent_sessions` uses a strict
`^\d+\.json$` filename guard so unrelated `*.json` files in the same dir are
never collected.

## Title Generation

`title_generator` calls the `Fast` model role after the first assistant turn
with a short prompt summarizer. The user toggles via `session.auto_title: bool`
in settings (see root CLAUDE.md multi-provider model-references rule — never a
bare model string).

## Timestamps

All timestamps in this crate (Session.created_at, TranscriptMetadata.{created,
modified}_at) are **milliseconds-since-epoch** as ASCII decimal strings —
`format!("{}", systemtime.as_millis())`. The cross-project newest-first sort
in `list_all_sessions` parses these as `u128` and compares; mixing seconds
with milliseconds silently mis-sorts. `timestamp_now()` is the canonical
emitter and MUST stay milliseconds.
