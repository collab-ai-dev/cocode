# coco-hub-server

Simplified local Event Hub server.

Routes and templates depend on `EventStore`, not on a concrete local file
reader. The default implementation is `LocalSessionJsonStore`, a read-only
projection over an injected `coco_session::SessionCatalog` that normalizes
session entries into Event Hub-like rows in memory. The default constructor
wires a disk `SessionCatalog` rooted at `<memory-base>`, but tests and future
backends can inject another catalog.

The `EventStore` model/query/error types are backend-agnostic and live under
`src/store/`. Local JSONL code must depend on those types; common store types
must not depend on `local_store`.

`SqliteEventStore` is the ingest-capable backend for Hub v2 batches. It owns
its SQLite connection behind the `EventStore` trait, enforces
`(instance_id, session_id, session_seq)` deduplication, and keeps fixed-field
indexes plus a serialized row projection for the HTTP/UI read model. The
`/v1/connect` WebSocket route accepts Hub v2 `announce` and `batch` frames and
routes them through `EventStore`. The standalone `coco-hub-server serve`
binary uses SQLite by default via `--data-dir` / `data/events.sqlite`; pass
`--memory-base` only for the read-only JSONL projection mode. SQLite
`run_retention_sweep` expires old events, prunes empty sessions, enforces a
database-size cap by dropping oldest sessions, and vacuums after size-cap
deletes. The standalone SQLite mode starts a periodic retention task controlled
by `--hub-retention-days`, `--hub-retention-max-bytes`, and
`--hub-retention-sweep-interval-secs`. The library-level
`serve_sqlite_until` / `serve_sqlite_listener_until` helpers own the shared
SQLite router + retention startup path used by both the standalone
`coco-hub-server serve` binary and the optional embedded `coco --serve-hub`
mode. The `/sse/session/{instance}/{session}` route subscribes to per-session
live topics and streams rendered event-row partials for newly accepted
WebSocket batches.

Key invariant: `coco-session` remains the source of truth. Store adapters may
derive synthetic Event Hub rows for UI/API compatibility, but simplified mode
must not write derived hub state beside the session backend.
