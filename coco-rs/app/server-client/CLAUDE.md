# coco-app-server-client

Remote typed client for AppServer. `lib.rs` owns the JSON-RPC/session core and
public error mapping, `remote_demux.rs` owns surface/lifecycle demultiplexing,
and `remote_transport.rs` owns NDJSON/WebSocket connection tasks and dialing.
The crate depends only on canonical DTOs and wire transport, never on the
server implementation. In-process client composition lives in
`coco-agent-host::local_client`.

## Invariants

- Surface event, lifecycle, and server-request delivery DTOs are owned by
  `coco-types`. The client and server share those values without a
  server-owned compatibility wrapper; lifecycle delivery is
  `SurfaceLifecycleEffect` directly, with one `surface_id`.
- Dependencies are limited to `coco-types`, `coco-app-server-transport`, and
  general-purpose async/serde libraries. Cross-crate server/client integration
  tests live in agent-host, which intentionally depends on both sides.
- Snapshot-required subscribe results are returned as
  `ClientError::SnapshotRequired`; no passive handle is minted unless the
  AppServer actually attached the surface.
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
  silently orphan a still-live session. Neither `RemoteSessionClient` nor
  `RemotePassiveSessionClient` is `Clone`, so consume-self replace/close is
  type-enforced (§14/H-4). The remote replace success paths call
  `RemoteEventDemux::purge_surface` on the replaced surface.
- `RemoteConnectOptions` names outbound and event channel capacities for
  remote NDJSON/Unix/WebSocket connections plus an optional `request_timeout`
  applied to every remote JSON-RPC request and an optional `write_timeout`
  applied to every outbound frame write. Defaults match the original fixed
  capacities with no request timeout and a 30s write timeout.
- Remote dialing failures are typed: `connect_unix`, `connect_named_pipe`,
  `connect_websocket`, and their `_with_options` / `_with_channel_capacity`
  variants return `ClientError::Connect` with the underlying transport error
  text preserved in the message.
- When `RemoteConnectOptions.request_timeout` is set,
  `RemoteJsonRpcClient::request` races the pending response against the
  timeout; on expiry it removes the pending correlation entry and returns
  `ClientError::Timeout`. A response arriving after the timeout hits the
  unknown-response-id contract and is tolerated-with-warn: `resolve_success` /
  `resolve_error` `tracing::warn!` and drop any unknown / late / duplicate /
  null response id instead of invalidating the connection. A late/duplicate
  reply is peer noise, not correlation corruption, and is never delivered to
  another request. The pending map is a `std::sync::Mutex` (every critical
  section — insert / remove / drain — is non-await) so `Drop` can resolve
  pending futures without an async context.
- `RemoteConnectOptions.write_timeout` bounds each outbound frame write in both
  owner loops (default 30s, mirroring the server's slow-consumer guard). On
  expiry the loop breaks with `RemoteTransportError::SlowConsumer`, which routes
  through the guaranteed-disconnect path. `None` disables the bound.
- `RemoteJsonRpcIncoming` decodes known `session/event` and
  `session/lifecycle` notifications into typed surface deliveries, preserves
  unknown notifications as raw JSON-RPC notifications, and surfaces inbound
  server-initiated JSON-RPC requests as `RemoteJsonRpcEvent::ServerRequest`.
  Server request frames must not invalidate the connection because
  approval/user-input/MCP callbacks use this direction. Notification payload
  decode failures (unknown lifecycle effect kinds / event layers on a newer
  server) are tolerate-with-warn: the notification is dropped, not fatal —
  a fire-and-forget delivery cannot corrupt correlation and this buys
  forward-compat. Only transport Io / FrameTooLarge / frame-level Decode,
  events-channel-closed, and write failures stay fatal.
- Remote disconnect is dual-channel: `RemoteJsonRpcIncoming::disconnect`
  resolves every pending RPC with `ClientError::Disconnected`, emits a terminal
  `RemoteJsonRpcEvent::Disconnected`, and invalidates later client requests
  with `ClientError::ClientInvalid`. Both owner `run()` loops break to a single
  post-loop `disconnect().await` on every exit path (no `?` short-circuits past
  it), and a `Drop` impl on `RemoteJsonRpcIncoming` re-runs the pending
  resolution + invalid flag if the owner task is aborted/dropped without a
  graceful disconnect — so the standard shutdown move still satisfies the
  dual-channel contract and no in-flight RPC hangs.
- `RemoteEventDemux::purge_surface` drops a surface's buffered events/lifecycle
  after it is closed/replaced or a `SessionEnded` for it is consumed; the
  connection-scoped `server_requests` / `notifications` queues are not
  surface-keyed and are bounded (drop-oldest + warn) instead.
- Remote JSON-RPC standard error codes map to typed public `ClientError`
  variants: `INVALID_REQUEST` -> `InvalidRequest`, `INVALID_PARAMS` ->
  `InvalidParams`, `METHOD_NOT_FOUND` -> `MethodNotFound`, and
  `INTERNAL_ERROR` -> `InternalServerError`. Connect-phase dialing failures
  are `ClientError::Connect`; a configured per-request timeout expiring is
  `ClientError::Timeout`. Stable domain payloads with
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

Interactive handles do not yet inject their stored `session_id` / `surface_id`
into turn and runtime-control requests because the canonical request DTOs lack
an explicit target. Consequently one connection cannot safely control multiple
interactive sessions even though event demux supports multiple surfaces. The
breaking remediation is specified in
`docs/coco-rs/multi-session-app-server/protocol-scope.md` and
`docs/coco-rs/multi-session-app-server/remediation-plan.md`.

Direct AppServer-owned persisted session-store listing/read semantics and
broader TUI/Hub cut-over remain pending follow-up work.
The client crate already exposes typed `session_list` / `session_read` request
helpers; today the CLI's runtime-backed AppServer bridge layers live session
visibility over the persisted handler response, while the CLI `SessionManager`
still supplies persisted transcript data for those requests.
