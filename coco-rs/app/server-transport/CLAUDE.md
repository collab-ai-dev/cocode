# coco-app-server-transport

Pure transport/wire-format layer for AppServer: JSON-RPC frame types + serde,
NDJSON per-record helpers, generic async reader/writer/duplex primitives, and
stdio / Unix-socket / Windows-named-pipe bindings with listener wrappers.

## Invariants

- **Wire framing only.** Must not own sessions, inspect transcripts, or
  depend on `coco-app-server`. JSON-RPC frame types live here (not in
  `coco-types`) — wire artifacts shared by the server-side `JsonRpcAdapter`
  and client-side remote transports.
- Frame ids preserve JSON-RPC's string/number/null id domain; notifications
  have no id. Request/notification/response frames preserve arbitrary JSON
  params/result/data without domain interpretation.
- NDJSON helpers encode exactly one JSON-RPC frame plus a trailing `\n` and
  decode exactly one already-delimited record (LF and CRLF accepted).
- **No spawned tasks.** `NdjsonFrameReader`/`NdjsonFrameWriter` operate over
  caller-owned async streams; they do not spawn tasks, open sockets, or bind
  process stdio. `NdjsonDuplexConnection` tracks local open/closed state and
  clean EOF only — no accept loops, reconnect, or AppServer surface cleanup;
  `split` hands the framed halves to the adapter layer for concurrent
  read/write ownership.
- Platform bindings (`ndjson_stdio_connection`; Unix `connect_ndjson_unix` /
  `bind_ndjson_unix_listener`; Windows named-pipe equivalents) wrap the same
  duplex framing. Listeners accept one framed connection at a time for
  caller-owned accept loops; the wrapped Unix listener owns the socket-file
  path and removes it on drop — `into_inner` transfers that responsibility
  to the caller.

## Pending

AppServer adapter integration, WebSocket ownership, slow-consumer
disconnects, and connection close cleanup live in `coco-app-server`;
process-level listener supervision belongs to the higher layer that owns
shutdown.
