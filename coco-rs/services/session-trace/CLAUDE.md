# coco-session-trace

Semantic execution-trace bundles for post-mortem debugging and golden replay.

`coco-wire-dump` captures raw LLM *traffic* (request/response bytes); this crate
captures execution *semantics* — tool lifecycle, compaction edges, turn
boundaries — as a small, stable, shareable artifact that survives a session and
replays deterministically offline.

## Pipeline

```
CoreEvent ──TraceEvent::from_core_event──▶ TraceEvent (semantic subset)
                                              │ TraceWriter::record_core
                                              ▼
                          <dir>/manifest.json + <dir>/trace.jsonl
                                              │ replay_bundle(<dir>)
                                              ▼
                          ReplayBundle { events, tool_calls, compaction_count }
```

## Key Types

- `TraceEvent` — semantic projection of `coco_types::CoreEvent` (tool
  queued/started/completed, MCP begin/end, turn started/ended, compaction
  started/compacted/failed). `from_core_event` returns `None` for events with no
  durable semantic meaning (text deltas, TUI-only events).
- `TraceManifest` / `SCHEMA_VERSION` — bundle header (schema version, session
  id, created stamp). The `created_unix_ms` is caller-supplied so tests stay
  deterministic.
- `TraceWriter` — append-only writer: `manifest.json` once, then one
  `TraceRecord` (`seq` + event) per line of `trace.jsonl`.
- `replay_bundle(dir)` → `ReplayBundle` — reads a bundle back and reduces it to
  a per-call `ToolCallStatus` map + a compaction count, for golden assertions.

## Integration

Non-invasive: a runner that already holds the `mpsc::Sender<CoreEvent>` sink can
tee each event into `TraceWriter::record_core(&event)`. This crate does not touch
the `QueryEngine` hot path. Error tier 3 (snafu + `coco-error`).
