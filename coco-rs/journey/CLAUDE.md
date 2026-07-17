# coco-journey

Read-side assembler for the `/journey` learning timeline: a pure disk scan +
journal merge that turns on-disk skill/memory state into a `JourneySnapshot`.
No I/O policy, no TUI, no domain mutation. Consumed only by the host layer
(`app/agent-host::session_dialogs`, driven by `app/cli`'s `/journey` dialog),
which converts to `coco_types` wire types — the TUI never sees these types,
and `/journey` mutations live in that host code, not here.

## Key Types

| Type | Purpose |
|------|---------|
| `build_journey` | Assembly entry point: agent-skill scan + used user/project skills + memory scan + telemetry join + journal merge. |
| `JourneyNode` / `JourneyNodeBody` / `AgentSkillLifecycle` | One timeline node. `Learning` carries `{invocations, required}` so no consumer re-derives the curator's promotion gate; invocations = successes **+** failures, matching that gate. |
| `bucketize` / `TimelineBucket` / `day_label` | Pure day→month→year adaptive bucketing with a recency ink signal; a span ≤ 32 days locks day granularity. |

## Invariants

- **Infallible**: a missing dir / corrupt file means that source contributes
  nothing (plus `tracing`) — never an error. Sync blocking I/O; async callers
  wrap in `spawn_blocking`.
- **Journal read-only.** Reads both journals via
  `coco_maintenance::journal::read_jsonl` at paths owned by the domain crates
  (`coco_skills::agent_scope::agent_journal_path`,
  `coco_memory::path::memory_journal_path`). Never writes — appends go through
  `coco_skill_learn::journal::append_event` / `coco_memory::journal::append_event`.
- Merge semantics: skill events are keyed by skill name; `MemoryWritten` fans
  out over its file list; `MemoryConsolidated` has no per-file key and is
  deliberately dropped.
- Timestamps: `first_seen = earliest journal event > provenance created_at >
  mtime`; `last_activity = max(journal, telemetry last_*_at, mtime)`.
- Deterministic time: the clock (`now_ms`) is injected — no `SystemTime`.
  Bucketing and labels are UTC, built from components (not strftime), so
  output never depends on the machine's timezone or locale.
- Agent skills come from `agent_scope::scan_agent_skills` (retired included);
  user/project skills from the caller's `SkillManager::all()` filtered to
  non-bundled, non-agent, actually-used — the split avoids double counting.
