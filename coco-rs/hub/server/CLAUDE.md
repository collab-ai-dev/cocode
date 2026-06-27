# coco-hub-server

Simplified local Event Hub server.

Routes and templates depend on `EventStore`, not on a concrete local file
reader. The default implementation is `LocalSessionJsonStore`, a read-only
adapter over `<memory-base>/projects/*/*.jsonl` that normalizes transcript
lines into Event Hub-like rows in memory.

The `EventStore` model/query/error types are backend-agnostic and live under
`src/store/`. Local JSONL code must depend on those types; common store types
must not depend on `local_store`.

This crate does not ingest WebSocket frames, write SQLite, or maintain
retention state in simplified mode.

Key invariant: transcript JSONL remains the source of truth. Store adapters may
derive synthetic Event Hub rows for UI/API compatibility, but simplified mode
must not write derived hub state beside the transcripts.
