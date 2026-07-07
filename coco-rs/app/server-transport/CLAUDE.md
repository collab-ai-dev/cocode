# coco-app-server-transport

Pure transport/wire-format layer for AppServer. This crate is currently a
Phase A foundation slice: it owns JSON-RPC frame types and serde behavior, but
does not yet implement UDS, WebSocket, connection acceptance, backpressure, or
adapter-side close cleanup. The current NDJSON support includes
per-record helpers, generic async reader/writer primitives, and a generic
duplex connection wrapper. Process stdio binding is available as a constructor
only; this crate still does not own async connection tasks or AppServer adapter
lifecycle.

## Invariants

- This crate contains wire framing only. It must not own sessions, inspect
  transcripts, or depend on `coco-app-server`.
- JSON-RPC frame types live here rather than in `coco-types`; they are wire
  artifacts shared by future server-side `JsonRpcAdapter` and client-side
  remote transports.
- Frame ids preserve JSON-RPC's string/number/null id domain. Notifications
  have no id.
- Request, notification, success response, and error response frames preserve
  arbitrary JSON params/result/data values without domain interpretation.
- NDJSON helpers encode exactly one JSON-RPC frame plus a trailing `\n` and
  decode exactly one already-delimited record, accepting LF and CRLF endings.
- `NdjsonFrameReader` / `NdjsonFrameWriter` operate over caller-owned async
  streams. They do not spawn tasks, open sockets, or bind process
  stdin/stdout.
- `NdjsonDuplexConnection` tracks local open/closed state and clean EOF over
  caller-owned streams. It does not implement accept loops, reconnect, or
  AppServer surface cleanup.
- `ndjson_stdio_connection` binds process stdin/stdout to the same duplex
  framing layer. It does not spawn or supervise a connection owner task.

## Pending

UDS/named-pipe transport, WebSocket framing, connection-owner tasks,
slow-consumer backpressure behavior, full adapter-side close cleanup, and
adapter integration remain pending Phase A work.
