# coco-app-server-client

Typed client-side handles for AppServer. This crate is currently a Phase A
foundation slice: it exposes an in-process `ServerClient` over
`coco_app_server::LocalClientAdapter` plus typed interactive/passive surface
handles, a transport-agnostic `RemoteJsonRpcClient` core, an NDJSON remote
connection loop over caller-owned streams, `RemoteConnectOptions` for named
remote channel capacities, a per-surface `RemoteSurfaceStream` facade over the
remote event demux, typed local and remote request helpers, and Unix-domain
socket dialing for future SDK transports.

## Invariants

- The client crate depends on `coco-app-server`; the server crate must not
  depend on the client crate.
- `ServerClient` owns one connection. Sequential and concurrent surfaces on
  that connection are represented by `SessionClient` and `PassiveSessionClient`
  handles.
- `SessionClient` and `PassiveSessionClient` expose typed `SessionId` and
  `SurfaceId` accessors; handles are not re-pointed to another session.
- `PassiveSessionClient` has no turn-start, interrupt, or replace methods.
- Snapshot-required subscribe results are returned as
  `ClientError::SnapshotRequired`; no passive handle is minted unless the
  AppServer actually attached the surface.
- The in-process foundation keeps transport receivers on `ServerClient`.
  `try_next_session_event` / `try_next_passive_event` and their async
  `next_*` counterparts demux the shared event receiver by `SurfaceId` and
  buffer other surfaces; the full owned stream API lands with the
  transport/client work.
- Server-request and lifecycle receivers follow the same `SurfaceId` demux
  rule. Reading one handle's request/lifecycle queue must not consume another
  surface's delivery on the same connection.
- `ServerClient::list_live_sessions` is the client-side live projection for
  future `list_sessions`: it returns `SessionId` plus current surface counts,
  not persisted transcript metadata.
- `ServerClient::detach_passive` consumes a passive handle and removes only
  that surface. It does not close the connection or archive the session.
- `ServerClient` typed helpers (`session_start`, `session_resume`,
  `session_list`, `session_read`, `session_archive`, `turn_start`,
  `turn_interrupt`, approval/user-input/elicitation resolve, `initialize`,
  config/runtime-control, MCP, plugin reload, and context-usage helpers)
  dispatch canonical `ClientRequest`s through a caller-supplied
  `LocalClientRequestHandler`. This is the typed local TUI/headless seam; it
  must stay in-process and avoid JSON-RPC framing.
- `RemoteJsonRpcClient` owns client-side JSON-RPC request id generation,
  pending-response correlation, and success/error replies to server-initiated
  JSON-RPC requests. Concrete transports own I/O and feed frames to
  `RemoteJsonRpcIncoming`.
- `RemoteJsonRpcClient` typed helpers (`session_start`, `session_resume`,
  `session_list`, `session_read`, `session_archive`, `turn_start`,
  `turn_interrupt`, approval/user-input/elicitation resolve, `initialize`,
  config/runtime-control, MCP, plugin reload, and context-usage helpers) are
  thin wrappers over canonical `ClientRequest` variants and decode existing
  `coco-types` result DTOs.
- `RemoteConnectOptions` names outbound and event channel capacities for
  remote NDJSON/Unix connections. Defaults match the original fixed capacities.
- `RemoteJsonRpcIncoming` decodes known `session/event` and
  `session/lifecycle` notifications into typed surface deliveries, preserves
  unknown notifications as raw JSON-RPC notifications, and surfaces inbound
  server-initiated JSON-RPC requests as `RemoteJsonRpcEvent::ServerRequest`.
  Server request frames must not invalidate the connection because
  approval/user-input/MCP callbacks use this direction.
- Remote disconnect is dual-channel: `RemoteJsonRpcIncoming::disconnect`
  resolves every pending RPC with `ClientError::Disconnected`, emits a terminal
  `RemoteJsonRpcEvent::Disconnected`, and invalidates later client requests
  with `ClientError::ClientInvalid`.
- `RemoteNdjsonConnection::run` is the client-side owner loop for caller-owned
  NDJSON streams. It multiplexes outbound JSON-RPC requests with inbound
  responses/notifications/server requests and performs the same disconnect
  invalidation on EOF or transport failure.
- On Unix, `RemoteJsonRpcClient::connect_unix` dials a local NDJSON Unix
  socket and returns the same client, connection owner, and mixed event
  receiver as `connect_ndjson`. The caller still owns spawning and supervising
  the returned connection owner.
- `RemoteEventDemux` wraps the mixed remote event receiver and provides
  synchronous and async per-surface event/lifecycle demux plus buffered
  server-request and raw-notification access. It is the foundation for the
  public stream facade.
- `RemoteSurfaceStream` is a borrowed per-surface facade over
  `RemoteEventDemux`; it does not own the connection receiver or permit
  concurrent mutable reads from multiple surface streams.

## Pending

Persisted session listing, concrete WS dialing, runtime-backed
start/resume/query/interrupt/replace/close semantics behind AppServer, and
typed public client errors for every server failure remain pending follow-up
work.
