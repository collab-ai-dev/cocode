# coco-app-server-client

Remote typed client for AppServer. `lib.rs` owns the JSON-RPC/session core and
public error mapping, `remote_demux.rs` owns surface/lifecycle
demultiplexing, and `remote_transport.rs` owns NDJSON/WebSocket connection
tasks and dialing. In-process client composition lives in
`coco-agent-host::local_client`.

## Invariants

- **Dependency boundary:** only `coco-types`, `coco-app-server-transport`,
  and general-purpose async/serde libraries — never the server
  implementation. Cross-crate server/client integration tests live in
  agent-host, which intentionally depends on both sides.
- Surface event, lifecycle, and server-request delivery DTOs are owned by
  `coco-types`; client and server share them with no compatibility wrapper.
  Lifecycle delivery is `SurfaceLifecycleEffect` directly, one `surface_id`.
- `RemoteJsonRpcClient` owns client-side JSON-RPC request-id generation,
  pending-response correlation, and replies to server-initiated requests.
  Its typed helpers (session lifecycle/read, turn, task, approval/user-input/
  elicitation resolve, initialize, config/runtime-control, MCP, plugin/hook
  reload, context usage) are thin wrappers over canonical `ClientRequest`
  variants decoding existing `coco-types` result DTOs.
- **Per-request targeting is implemented:** `RemoteSessionClient` helpers set
  `params.target = self.interactive_target()` on turn and runtime-control
  requests, so one connection can control multiple interactive sessions.
- `RemoteSessionClient` (query/interrupt/replace/close) and
  `RemotePassiveSessionClient` (snapshots + events only) are immutable
  handles reading through `RemoteEventDemux`. Start/resume helpers mint
  handles directly from the required `surface_id` on the result DTO — a
  successful lifecycle call always attaches an interactive surface, so there
  is no optional-surface fallback.
- Replace/close **consume self** and return the original handle on failure so
  callers cannot silently orphan a live session; neither handle is `Clone`,
  making this type-enforced. Remote replace success calls
  `RemoteEventDemux::purge_surface` on the replaced surface.
- `subscribe_session` is the passive remote attach path: it returns a
  `RemotePassiveSessionClient` only after AppServer actually attaches the
  surface, preserving replayed envelopes on the handle. Snapshot-required
  replies map to `ClientError::SnapshotRequired` — **no fake handle is ever
  minted**; the caller reads a snapshot and subscribes again.
- `RemoteConnectOptions` names outbound/event channel capacities plus an
  optional `request_timeout` (every remote request) and `write_timeout`
  (every outbound frame write; default 30s, mirroring the server's
  slow-consumer guard; `None` disables). On write timeout the owner loop
  breaks with `RemoteTransportError::SlowConsumer` via the
  guaranteed-disconnect path.
- On `request_timeout` expiry the pending correlation entry is removed and
  `ClientError::Timeout` returned. A response arriving late hits the
  unknown-response-id contract and is **tolerated-with-warn**:
  `resolve_success`/`resolve_error` warn and drop unknown/late/duplicate/null
  response ids instead of invalidating the connection — peer noise, not
  correlation corruption; never delivered to another request.
- The pending map is a `std::sync::Mutex` — every critical section
  (insert/remove/drain) is non-await — so `Drop` can resolve pending futures
  without an async context.
- `RemoteJsonRpcIncoming` decodes known `session/event` / `session/lifecycle`
  notifications into typed surface deliveries, preserves unknown
  notifications raw, and surfaces inbound server-initiated requests as
  `RemoteJsonRpcEvent::ServerRequest` (these must not invalidate the
  connection — approval/user-input/MCP callbacks use this direction).
  **Error taxonomy:** notification payload decode failures (unknown
  lifecycle-effect kinds / event layers from a newer server) are
  tolerate-with-warn — a fire-and-forget delivery can't corrupt correlation,
  and this buys forward-compat. Only transport Io / FrameTooLarge /
  frame-level Decode, events-channel-closed, and write failures stay fatal.
- **Dual-channel disconnect:** `RemoteJsonRpcIncoming::disconnect` resolves
  every pending RPC with `ClientError::Disconnected`, emits a terminal
  `RemoteJsonRpcEvent::Disconnected`, and invalidates later requests with
  `ClientError::ClientInvalid`. Both owner `run()` loops break to a single
  post-loop `disconnect().await` on every exit path (no `?` short-circuits
  past it), and a `Drop` impl re-runs the pending resolution + invalid flag
  if the owner task is aborted without a graceful disconnect — no in-flight
  RPC ever hangs.
- Error mapping: JSON-RPC standard codes map to typed `ClientError` variants
  (`InvalidRequest`, `InvalidParams`, `MethodNotFound`,
  `InternalServerError`); dialing failures are `ClientError::Connect` with
  transport error text preserved. Stable domain payloads with `data.kind`
  map to narrower variants (`snapshot_required` -> `SnapshotRequired`,
  `surface_limit` -> `SurfaceLimit`); unknown domain kinds stay
  `ClientError::Domain { code, kind, message, data }`; unknown codes without
  a kind stay `ClientError::Server { code, message, data }`.
- Transport owner loops: `RemoteNdjsonConnection::run` (caller-owned NDJSON
  streams) and `RemoteWebSocketConnection::run` multiplex outbound requests
  with inbound responses/notifications/server requests and perform the same
  disconnect invalidation on EOF/failure. `connect_unix` (Unix),
  `connect_named_pipe` (Windows), and `connect_websocket` all return the
  same `(client, owner, events)` shape as `connect_ndjson`; the caller owns
  spawning and supervising the owner.
- `RemoteEventDemux` wraps the mixed event receiver: sync/async per-surface
  event/lifecycle demux plus buffered server-request and raw-notification
  access. `purge_surface` drops a surface's buffered events/lifecycle after
  close/replace or a consumed `SessionEnded`. `RemoteSurfaceStream` is a
  borrowed per-surface facade (no concurrent mutable reads across streams);
  `RemoteOwnedSurfaceStream` owns the demux for single-surface callers while
  still exposing it for server requests and other buffered surfaces.

## Per-surface buffer bound

`RemoteEventDemux` caps each surface's buffered events/lifecycle at
`MAX_BUFFERED_SURFACE_QUEUE`. Unlike the connection-scoped
`server_requests` / `notifications` queues (bounded, drop-oldest + warn), it
does **NOT** drop-oldest — an event stream is ordered and lifecycle drops
desync surface state — so overflow means the caller is not draining a
subscribed surface, and the demux disconnects (`disconnected = true`); the
caller reconnects and re-snapshots. `RemoteJsonRpcIncoming::handle_frame`
applies the same policy at the connection events channel: `try_send`, and on
a full channel `ClientError::SlowConsumer` routes through the
guaranteed-disconnect path so pending RPCs resolve instead of hanging.

## Pending

Direct AppServer-owned persisted session-store listing/read semantics remain
follow-up work: the client already exposes typed `session_list` /
`session_read` helpers, while the CLI's runtime-backed AppServer bridge
layers live-session visibility over the persisted handler response.
