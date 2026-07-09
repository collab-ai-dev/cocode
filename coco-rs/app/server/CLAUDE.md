# coco-app-server

App-server ownership and routing layer. This crate is currently a Phase A
foundation slice: it owns connection/surface routing metadata, replay-ring
behavior, and the registry slot-state skeleton, but does not yet own runtime
construction or transports.

## Key Types

| Type | Purpose |
|------|---------|
| `AppServer` | Owns registry + routing locks and combined no-await commit sections. |
| `AppLoadStart` | Result of starting/observing a load owner task. |
| `AppCloseStart` | Result of starting/observing a close owner task. |
| `AppReplaceStart` | Result of starting a replace owner task. |
| `AppArchiveCommit` | Result of completing close and archiving surfaces in one commit section. |
| `AppLiveSessionSummary` | Live registry session id plus current routing surface counts. |
| `SurfaceLifecycleEffect` | Internal lifecycle effect targeted to a surface after commit. |
| `SurfaceLifecycleDelivery` | Lifecycle effect delivery queued to one target surface. |
| `ServerRequestRouteOutcome` | Result of routing one server-initiated request through the AppServer request bridge. |
| `LocalClientAdapter` | Typed in-process adapter that registers real AppServer connections and channels. |
| `LocalClientConnection` | One local connection with event, request, and lifecycle receivers. |
| `LocalClientRequestHandler` | Runtime-supplied bridge for handling typed in-process `ClientRequest`s from local clients. |
| `LocalClientSubscribeOutcome` | Local subscribe result: attached replay or snapshot-required without attachment. |
| `JsonRpcAdapter` | Remote adapter foundation that registers real AppServer connections and owns JSON-RPC server-request correlation. |
| `JsonRpcAdapterConnection` | One remote adapter connection with event, request, lifecycle receivers, and pending JSON-RPC response ids. |
| `JsonRpcRequestHandler` | Runtime-supplied bridge for handling typed `ClientRequest`s decoded from JSON-RPC requests. |
| `LiveSessionRegistry` | Slot-state registry for root sessions: `Loading`, `Live`, `Closing`. |
| `LoadCompletion` / `CloseCompletion` | Cloneable completion signals; owner tasks do the work and update slots. |
| `ReplaceStart` / `ReplaceCommit` | Registry-side replace reservation and commit results. |
| `SessionDataProjectionError` / `SessionPage` / `TranscriptTurnEntry` | Pure cursor, pagination, and turn-span projection helpers for session-data reads. |
| `ConnectionKey` | Private in-process transport key. Never serialize or persist it. |
| `RoutingState` | Single-lock state for connection/surface indexes and per-session durable rings. |
| `SurfaceAttachment` | Server-owned attachment metadata: role, capabilities, notification prefs, delivery cursor, state. |
| `SurfaceRole` | `Interactive` or `Passive`; exactly one interactive owner per session. |
| `AttachError` | Snafu-backed attach failure (`InteractiveOwnerConflict`, `SurfaceLimit`, `SessionClosing`). |
| `SurfaceDelivery` | One envelope delivery targeted to one `SurfaceId`. |
| `ServerRequestDelivery` | Actionable server->client request targeted to one `SurfaceId`. |
| `ServerRequestReply` / `ResolvedServerRequest` | Client reply payload plus resolved pending-request ownership metadata. |
| `SubscribeReplay` | Result of `after_seq` replay lookup: replay events or require a snapshot. |
| `RouteOutcome` | Delivery count plus connections disconnected for full/closed outbound queues. |
| `DetachSurfaceOutcome` | Result of removing one surface from its owning connection. |
| `SessionSurfaceCounts` | Attached/closed surface counts for a session routing projection. |
| `PendingServerRequest` | Server->client request ownership metadata keyed by monotonic request id. |
| `PendingServerRequestReplay` | Retained pending metadata plus actionable request payload for replay to a surface. |
| `ReplaceSurfaceOutcome` | Result of re-pointing one caller surface during session replace. |
| `ArchiveSessionOutcome` | Surfaces closed by session archive routing. |

## Invariants

- `ConnectionKey` is internal only: no wire format, no disk format, no client API.
- `LiveSessionRegistry` stores only lifecycle slots. Runtime construction and
  close cascade run in owner tasks that call `complete_load_*` /
  `complete_close`.
- `AppServer::spawn_load` is the load owner-task entry point. Only the caller
  that reserves a fresh `Loading` slot spawns the factory future; later callers
  observe the same completion signal and their factories are dropped unpolled.
- `AppServer::spawn_close` is the live-session close owner-task entry point. It
  marks `Live -> Closing`, runs the supplied close cascade future in a spawned
  task, then completes archive routing and removes the slot.
- `spawn_close` on a `Loading` slot records a close-after-load request in the
  slot. Load failure completes the close signal immediately; load success moves
  directly into `Closing` and the single close owner task runs the supplied
  cascade.
- `AppServer::spawn_replace` is the surface-aware replace owner-task entry
  point. It reserves the replacement as `Loading`, runs the construction
  future, commits the registry+routing swap on success, then runs the supplied
  old-session close cascade and archive completion. Construction failure
  removes only the replacement slot and leaves old live.
- `AppServer::spawn_replace_detached` is the same owner-task lifecycle without
  caller-surface routing. Use it only when the caller will attach a fresh
  surface after the replacement commits.
- Owner tasks route lifecycle effects through `route_lifecycle_effects` after
  commit locks are released: replace emits started/replaced before the old close
  cascade, and close/archive emits ended after archive commit.
- On Unix, `JsonRpcAdapter::bind_and_run_unix_listener_until_shutdown` binds
  an NDJSON Unix socket listener, runs the supervised accept loop, and relies on
  the transport listener wrapper to remove the socket path when shutdown drops
  the listener.
- `Loading`, `Live`, and `Closing` all count toward `max_sessions`; `get` and
  `list_live` expose only `Live` handles.
- `Closing` keeps the session handle for the close supervisor but
  `begin_load`/resume paths receive only a close completion signal, never the
  draining handle.
- `begin_replace` requires old `Live`, reserves the replacement id as
  `Loading`, and bypasses `max_sessions` by exactly one slot for that swap.
- `complete_replace_success` is the registry-only half of Stage 2: new
  `Loading -> Live`, old `Live -> Closing`, with no await. The AppServer layer
  still owns taking the routing lock and re-pointing the caller surface in the
  same commit section.
- `AppServer::commit_replace_for_surface` takes the registry lock before the
  routing lock and performs the combined replace commit in one synchronous
  section: registry new `Loading -> Live`, old `Live -> Closing`, then routing
  caller old -> new and peer closure.
- `AppServer::complete_close_and_archive_surfaces` is the supervisor-completion
  commit: it requires a `Closing` slot, takes registry then routing locks,
  archives the session's surfaces, completes the close signal, and removes the
  registry slot.
- Commit methods return `SurfaceLifecycleEffect`s describing the post-commit
  lifecycle messages to send (`SessionStarted`, `SessionReplaced`,
  `SessionEnded`). Transport/wire emission happens after locks are released.
- Lifecycle effects use a separate per-connection lifecycle channel.
  `route_lifecycle_effects` is called after commit locks are released and can
  still target surfaces moved to `SessionClosed` because connection cleanup
  metadata is preserved until disconnect.
- Keep the four routing maps in sync:
  `SurfaceId -> SessionId`, `SessionId -> SurfaceId set`,
  `SurfaceId -> ConnectionKey`, `ConnectionKey -> SurfaceId set`.
- Keep `SurfaceAttachment` and `interactive_owners` in sync with those maps.
- Passive surfaces can share a session; a second interactive surface returns
  `InteractiveOwnerConflict` with owner metadata. Takeover is not implemented.
- `sole_interactive_session_for_connection` returns a session id only when the
  connection has exactly one attached interactive surface. It deliberately
  returns `None` for zero or multiple interactive surfaces so higher layers do
  not recreate a per-connection active-session default.
- `subscribe` must read the retention ring and attach the surface in one
  `RoutingState` mutation so replay-to-live has no gap.
- Only durable `SessionEnvelope`s enter the ring. Ephemeral envelopes are
  delivered live only.
- Honor `NotificationPrefs` per surface before queueing delivery.
- Server-initiated requests are routed only to the interactive surface that
  declared the required `SurfaceCapability`.
- Server-initiated requests use a separate per-connection request channel from
  the envelope/event channel. `route_server_request` records pending ownership
  and `try_send`s the actionable request to that channel.
- Routed server-initiated request payloads are retained only while their
  pending ownership is open, so late attach/replay can reconstruct actionable
  requests. Completing or cancelling a pending request removes the retained
  payload.
- `AppServer::resolve_server_request` validates reply `(session_id, request_id)`
  against pending ownership and clears the pending indexes before returning the
  reply payload to the future runtime/adapter bridge.
- `AppServer::route_server_request` and
  `pending_server_request_replays_for_surface` are the adapter-facing request
  bridge: adapters should not split request delivery/replay ownership outside
  the AppServer layer.
- `LocalClientAdapter` is typed and in-process only. It registers a real
  `ConnectionKey`, event sender, request sender, and lifecycle sender through
  AppServer and then attaches/subscribes surfaces through the same routing
  rules as remote adapters.
- `LocalClientConnection::dispatch_client_request` delegates canonical
  `ClientRequest`s to a runtime-supplied `LocalClientRequestHandler` with the
  connection context. This is the local typed request seam for TUI/headless
  clients; it must not serialize through JSON-RPC.
- `LocalClientConnection::subscribe_surface` returns `SnapshotRequired` without
  a surface id when the replay cursor cannot attach. Clients must read a
  snapshot and subscribe again; they must not receive a fake surface handle.
- `LocalClientConnection::detach_surface` removes only a surface owned by that
  connection. It does not close the connection, archive the session, or detach
  other surfaces on the same connection.
- `JsonRpcAdapter` registers the same event, server-request, and lifecycle
  channels as the local adapter, but keeps wire framing and response
  correlation in JSON-RPC space. It delegates decoded `ClientRequest`s to a
  runtime-supplied `JsonRpcRequestHandler`; it must not own runtime
  construction or parse transcripts.
- `JsonRpcAdapterConnection::encode_server_request` converts actionable
  `ServerRequestDelivery` payloads into JSON-RPC request frames and records
  the `JsonRpcId -> (SurfaceId, RequestId)` correlation. Responses are matched
  back to that pending metadata and resolved through AppServer's typed
  `ServerRequestReply` bridge before runtime code consumes them.
- `JsonRpcAdapterConnection::run_ndjson_transport` is the remote connection
  owner loop for caller-supplied NDJSON streams. It dispatches inbound client
  requests, emits event/server-request/lifecycle frames, and disconnects the
  AppServer connection on EOF or transport failure.
- `JsonRpcAdapterConnection::run_websocket_transport` is the equivalent owner
  loop for an already-accepted `tokio_tungstenite::WebSocketStream`. It maps
  text/binary WebSocket messages to `JsonRpcFrame`s, emits JSON-RPC responses
  and notifications as text messages, ignores ping/pong frames, and disconnects
  the AppServer connection on close or transport failure.
- On Unix, `JsonRpcAdapter::accept_unix_connection` accepts one framed Unix
  socket connection from a caller-owned listener and spawns its JSON-RPC owner
  task.
- On Unix, `JsonRpcAdapter::run_unix_listener_until_shutdown` owns a local
  supervised accept loop for a provided `NdjsonUnixListener`: it accepts
  framed connections, spawns one JSON-RPC owner task per connection, stops
  accepting on a shutdown signal, and waits for accepted owners to finish.
- On Unix, `JsonRpcAdapter::bind_and_run_unix_listener_until_shutdown` also
  owns binding and socket-file cleanup for the same supervisor. Higher layers
  still own process startup/configuration and shutdown signal selection.
- On Windows, `JsonRpcAdapter::run_named_pipe_listener_until_shutdown` owns
  the equivalent supervised accept loop for a provided
  `NdjsonNamedPipeListener`; `bind_and_run_named_pipe_listener_until_shutdown`
  binds the named pipe and runs that supervisor.
- `JsonRpcAdapter::run_websocket_listener_until_shutdown` owns the equivalent
  supervised accept loop for a caller-bound TCP listener: it accepts WebSocket
  handshakes, spawns one JSON-RPC owner task per connection, stops accepting on
  a shutdown signal, and waits for accepted owners to finish.
- `JsonRpcAdapterConnection::run_frame_channels` is the same remote connection
  owner loop over caller-supplied JSON-RPC frame channels. Higher layers use it
  to bridge existing transports into AppServer without moving concrete I/O into
  this crate.
- JSON-RPC remote owner loops apply a bounded outbound write/send timeout.
  NDJSON and WebSocket transports fail the owner with
  `JsonRpcConnectionOwnerError::TransportSlowConsumer`; frame-channel bridges
  fail with `JsonRpcAdapterError::SlowConsumer`. The owner disconnects the
  AppServer connection before returning the slow-consumer error.
- `JsonRpcAdapterConnection::app_server` exposes the owning `Arc<AppServer<_>>`
  to higher-layer bridge code that must keep lifecycle registration on the same
  registry/routing instance as the JSON-RPC connection.
- `AppServer::list_live_sessions` is a live-only projection for future
  `session/list` plumbing. It snapshots registry live slots with routing
  surface counts under registry-then-routing lock order; persistent transcript
  summaries still belong to the future runtime/session-store bridge.
- `parse_session_data_cursor`, `parse_session_data_limit`,
  `session_data_page`, `page_session_items`, and
  `derive_session_turn_summaries` are pure projection helpers for
  `session/read` / `session/turns/list`. They do not read transcripts or depend
  on `coco-session`; higher layers adapt their storage records into
  `TranscriptTurnEntry`.
- Keep pending-request indexes in sync by request, session, surface, and turn.
  Surface detach, connection close, turn transition, replace, and archive must
  cancel the precise affected request ids.
- `replace_calling_surface` re-points only the caller to the new session and
  moves old peers to `SessionClosed`; it does not auto-attach peers to the new
  session.
- `archive_session` moves every live surface on that session to
  `SessionClosed` and removes them from fan-out while keeping connection-side
  cleanup possible.
- Outbound queues are bounded by their transport owner. `RoutingState` uses
  `try_send`; full or closed queues disconnect the whole `ConnectionKey`.

## Pending

Concrete runtime factory implementation behind every `AppServer::spawn_load`
caller, concrete close cascade implementation behind `spawn_close`, concrete
replace runtime factory and old-session close cascade behind `spawn_replace`,
interactive takeover, production listener lifecycle wiring beyond the SDK
sidecars, and broader persisted transcript/session-store
integration are not implemented here yet. The CLI bridge now constructs the
initial TUI/headless/SDK runtime through `AppServer::spawn_load`, registers
remaining `LocalAppSessionHandle` snapshots through `spawn_load`, exposes
runtime-backed replace helpers that construct replacement handles inside
`spawn_replace` / `spawn_replace_detached` and return those handles to callers,
uses the runtime replacement context for production SDK `session/start` so the
client-started runtime is built through AppServer load/replace before the
startup placeholder slot closes, uses runtime-backed
`AppServer::spawn_replace` / `spawn_replace_detached` for TUI `/resume` and
`/branch` runtime construction, uses the same runtime-backed replacement
ordering for production SDK `session/resume`, archives through `spawn_close`,
and installs the runtime
`SessionManager` so local
`session/list` / `session/read` can read persisted transcripts. The local
handle snapshots can carry the fused app/cli `SessionHandle`, but their close
cascade remains retarget-safe and does not tear down the fused runtime until
Phase B removes in-place runtime retargeting. Runtime-backed handles are now
also the preferred live data source for unpersisted local
`session/list` / `session/read` / `session/turns/list` fallback results; SDK
snapshot and SDK-only handles read keyed SDK state instead of singleton
identity state.
Re-installing a runtime-backed handle for an already-live local session
refreshes the registry handle without changing surface routing; this is
lifecycle and local-handler wiring, not the final broad server-client
pagination bridge. The SDK production resume bridge now swaps SDK-visible
runtime/session state only after AppServer construction and replacement commit,
and rebuilds the SDK handoff from the resumed transcript plus runtime app
state. Persisted/live session-data composition is
centralized in the CLI bridge's local session-data view for now; this crate
still does not depend on `coco-session`, but it now owns the pure cursor,
pagination, and turn-span projection helpers shared by the local bridge and
legacy SDK session-data handlers. That bridge view reads the installed
`SessionManager` directly for local AppServer `session/list`, `session/read`,
and `session/turns/list` instead of bouncing those read-only methods through
the legacy SDK session-data handlers.
