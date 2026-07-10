# coco-app-server-client

Typed client-side handles for AppServer. This crate is currently a Phase A
foundation slice: it exposes an in-process `ServerClient` over
`coco_app_server::LocalClientAdapter` plus typed interactive/passive surface
handles, a transport-agnostic `RemoteJsonRpcClient` core, an NDJSON remote
connection loop over caller-owned streams, a WebSocket remote connection loop
over accepted/dialed streams, `RemoteConnectOptions` for named
remote channel capacities, typed remote interactive/passive surface handles
over known `(session_id, surface_id)` pairs, a per-surface
`RemoteSurfaceStream` facade over the remote event demux, typed local and
remote request helpers, Unix-domain socket dialing, and Windows named-pipe dialing for future SDK
transports.

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
  `session_list`, `session_read`, `session_turns_list`, `session_archive`,
  `session_cost`, `session_status`, `task_list`, `task_detail`,
  `background_all_tasks`, `turn_start`, `turn_interrupt`,
  approval/user-input/elicitation resolve, `initialize`,
  config/runtime-control, MCP, plugin/hook reload, and context-usage helpers)
  dispatch canonical `ClientRequest`s through a caller-supplied
  `LocalClientRequestHandler`. This is the typed local TUI/headless seam; it
  must stay in-process and avoid JSON-RPC framing.
- `ServerClient::query_session`, `interrupt_session`, `close_session`,
  `replace_session_with_start`, `replace_session_with_resume`,
  `read_passive_session`, and `list_passive_session_turns` are the local
  handle-oriented facade over those helpers while `ServerClient` still owns the
  shared connection receivers. Replace and close consume the interactive handle
  and return it on failure.
- `ServerClient::stop_task` is the local runtime-control helper for
  task/subagent cancellation. Handler implementations should target a real task
  registry when one is installed instead of treating it only as a turn
  interrupt.
- `ServerClient::config_apply_flags` carries runtime flag updates; local
  handlers currently apply `fast_mode` / `fastMode` to the installed session
  runtime and may acknowledge unknown flags for SDK compatibility.
- `ServerClient::set_thinking` carries the session thinking override; local
  handlers apply it to the installed runtime and emit `ModelRoleChanged` for
  TUI mirrors.
- `ServerClient::set_model_role` carries in-memory model-role overrides used
  by the TUI `/model` picker; local handlers apply the installed runtime
  override and emit `ModelRoleChanged`.
- `ServerClient::apply_permission_update` carries `/permissions` editor
  updates; local handlers apply the installed runtime's live permission base
  and persist destinations that map to settings files.
- `RemoteJsonRpcClient` owns client-side JSON-RPC request id generation,
  pending-response correlation, and success/error replies to server-initiated
  JSON-RPC requests. Concrete transports own I/O and feed frames to
  `RemoteJsonRpcIncoming`.
- `RemoteJsonRpcClient` typed helpers (`session_start`, `session_resume`,
  `session_list`, `session_read`, `session_turns_list`, `session_archive`,
  `session_cost`, `session_status`, `task_list`, `task_detail`,
  `background_all_tasks`, `turn_start`, `turn_interrupt`,
  approval/user-input/elicitation resolve, `initialize`,
  config/runtime-control, MCP, plugin/hook reload, and context-usage helpers)
  are thin wrappers over canonical `ClientRequest` variants and decode
  existing `coco-types` result DTOs.
- `RemoteSessionClient` and `RemotePassiveSessionClient` are immutable handles
  for remote surface attachments. They expose typed identity and read
  events/lifecycle through `RemoteEventDemux`; `RemoteSessionClient` owns
  query/interrupt/replace/archive helpers, while `RemotePassiveSessionClient`
  only reads snapshots and events. Remote start/resume handle helpers must mint
  handles from a server-provided `SurfaceId`: prefer the optional
  `surface_id` on the result DTO and fall back to the matching lifecycle
  activation for older streams.
- `RemoteJsonRpcClient::subscribe_session` is the passive remote attach path.
  It dispatches `session/subscribe`, returns a `RemotePassiveSessionClient`
  only after AppServer attaches the passive surface, preserves replayed
  envelopes on the handle, and maps snapshot-required replies to
  `ClientError::SnapshotRequired`.
- `RemoteSessionClient::replace_with_start`,
  `RemoteSessionClient::replace_with_resume`, and `RemoteSessionClient::close`
  consume the handle and return the original handle on failure so callers cannot
  silently orphan a still-live session.
- `RemoteConnectOptions` names outbound and event channel capacities for
  remote NDJSON/Unix/WebSocket connections. Defaults match the original fixed
  capacities.
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
- Remote JSON-RPC standard error codes map to typed public `ClientError`
  variants: `INVALID_REQUEST` -> `InvalidRequest`, `INVALID_PARAMS` ->
  `InvalidParams`, `METHOD_NOT_FOUND` -> `MethodNotFound`, and
  `INTERNAL_ERROR` -> `InternalServerError`. Stable domain payloads with
  `data.kind` map to narrower variants (`snapshot_required` ->
  `ClientError::SnapshotRequired`, `surface_limit` ->
  `ClientError::SurfaceLimit`); unknown domain kinds stay preserved as
  `ClientError::Domain { code, kind, message, data }`. Unknown
  server/application codes without a kind remain
  `ClientError::Server { code, message, data }`.
- `RemoteNdjsonConnection::run` is the client-side owner loop for caller-owned
  NDJSON streams. It multiplexes outbound JSON-RPC requests with inbound
  responses/notifications/server requests and performs the same disconnect
  invalidation on EOF or transport failure.
- `RemoteWebSocketConnection::run` is the equivalent owner loop for
  `tokio_tungstenite::WebSocketStream`s. `RemoteJsonRpcClient::connect_websocket`
  dials a WebSocket URL and returns the same `(client, owner, events)` shape as
  NDJSON/Unix helpers.
- On Unix, `RemoteJsonRpcClient::connect_unix` dials a local NDJSON Unix
  socket and returns the same client, connection owner, and mixed event
  receiver as `connect_ndjson`. The caller still owns spawning and supervising
  the returned connection owner.
- On Windows, `RemoteJsonRpcClient::connect_named_pipe` dials a local NDJSON
  named pipe and returns the same client, connection owner, and mixed event
  receiver as `connect_ndjson`. The caller still owns spawning and supervising
  the returned connection owner.
- `RemoteEventDemux` wraps the mixed remote event receiver and provides
  synchronous and async per-surface event/lifecycle demux plus buffered
  server-request and raw-notification access. It is the foundation for the
  public stream facade.
- `RemoteSurfaceStream` is a borrowed per-surface facade over
  `RemoteEventDemux`; it does not own the connection receiver or permit
  concurrent mutable reads from multiple surface streams.
- `RemoteOwnedSurfaceStream` owns a `RemoteEventDemux` for callers that want a
  single-surface event/lifecycle facade without carrying a separate demux
  borrow. It still exposes the underlying demux for server requests,
  notifications, and other buffered surfaces.

## Pending

Direct AppServer-owned persisted session-store listing/read semantics and
broader TUI/Hub cut-over remain pending follow-up work.
The client crate already exposes typed `session_list` / `session_read` request
helpers; today the CLI's runtime-backed AppServer bridge layers live session
visibility over the persisted handler response, while the CLI `SessionManager`
still supplies persisted transcript data for those requests.
