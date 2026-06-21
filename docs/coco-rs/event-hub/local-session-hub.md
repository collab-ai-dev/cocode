# Local Session Hub

This is the simplified Event Hub implementation in `coco-rs/hub/server`.

It reuses the Event Hub route shape and timeline-oriented UI, but swaps the
remote ingest/storage architecture for a read-only adapter over existing
session transcripts:

- Source: `<memory-base>/projects/<project-slug>/<session-id>.jsonl`
- UI: Axum routes + Askama templates under `coco-rs/hub/server/templates/`
- HTMX: `/p/events` serves the session timeline fragment for structured filters
- Static assets: served from `coco-rs/hub/server/web/static/`
- Vendor assets: committed HTMX, Flowbite JS, Prism core/theme, and Prism JSON
- No WebSocket connector
- No SQLite or derived storage
- No retention task
- Synthetic `instance_id`: the project slug directory name
- Synthetic event `seq`: `line_number * 1000 + content_block_index`, so one
  JSONL transcript entry can render multiple timeline rows

## Store Adapter Shape

The UI/API layer talks to `EventStore`. The simplified server wires that
trait to `LocalSessionJsonStore`, which reads JSONL on demand and emits
normalized rows. Store model/query/error types are backend-agnostic and live
under `coco-rs/hub/server/src/store/`; local JSONL parsing is just one backend.
The important boundary is:

```text
routes/templates
  -> EventStore
       -> LocalSessionJsonStore      # simplified, direct JSONL reads
       -> future SQLite/remote store # full Event Hub implementation
```

This means local mode is not a second frontend and not an ingest pipeline. It
is a base session JSON store adapter that simulates the Event Hub read model.
The conversion is per request and in memory only.

The simplified implementation intentionally keeps HTML and CSS out of Rust
source files. Handlers build view models and render templates; static CSS is
read from disk at request time so UI-only edits do not require changing Rust
code.

## Format Differences

Remote Event Hub events are envelopes:

```json
{
  "instance_id": "uuid",
  "session_id": "uuid",
  "seq": 42,
  "ts": "2026-05-16T12:34:56Z",
  "schema_version": 1,
  "payload": { "kind": "protocol", "...": "..." }
}
```

Local transcripts are JSONL records without an envelope. Message rows use
`type` values such as `user`, `assistant`, `system`, `attachment`, and
`tool_result`; metadata rows use `type` values such as `custom-title`,
`tag`, `last-prompt`, `cost-summary`, and compaction markers.

The local hub maps those rows to Event Hub-like rows for API/UI
compatibility:

- `kind = "transcript"` for `TranscriptEntry`
- `kind = "metadata"` for `MetadataEntry`
- `inner_kind = raw.type`
- `payload = raw JSONL object`
- `ts = parsed TranscriptEntry.timestamp` as unix milliseconds when present
- `ts_display = raw TranscriptEntry.timestamp` for UI display
- `role`, `msg_type`, `lane`, `file_refs`, and similar audit fields are
  normalized in the store adapter before the UI sees the row
- token/cost rollups are derived from transcript entry usage fields
