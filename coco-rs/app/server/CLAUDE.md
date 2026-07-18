# coco-app-server

App-server ownership and routing layer: connection/surface routing, replay
rings, lifecycle slots, protocol adapters, listener supervision. Runtime
construction stays behind higher-layer handler/factory traits. Test suites
live beside their modules (`routing.test.rs`, `app_server.test.rs`,
`json_rpc_adapter.test.rs`).

## Key Types

| Type | Purpose |
|------|---------|
| `AppServer` | Owns registry + routing locks and combined no-await commit sections |
| `SessionSeqAllocator` | Process-shared durable `session_seq` allocator: strictly monotonic per session across every forwarder path; watermark persist hook + `initialize_after_watermark` skip-ahead for cross-restart continuity |
| `LiveSessionRegistry` | Slot-state registry for root sessions: `Loading` / `Live` / `Closing` |
| `SessionRegistrationPolicy` (`SessionTopology` / `SessionVisibility` / `SessionEgress`) | Product-neutral immutable per-slot metadata fixed at reservation (`root()` → Root/Public/DurableHub; `side_chat_child(parent)` → Child/Internal/LocalOnly). This is the authoritative AppServer topology model. |
| `RoutingState` | Single-lock state for connection/surface indexes and per-session durable rings |
| `SurfaceAttachment`, `SurfaceRole` | Attachment metadata (role, capabilities, notification prefs, delivery cursor, state); `Interactive` or `Passive`, at most one interactive owner per session |
| `AttachError` | Snafu-backed attach failure (`InteractiveOwnerConflict`, `SurfaceLimit`, `SessionClosing`, `SessionNotFound`) |
| `ReplaceCommitFailure<H>` | `commit_replace_for_surface` failure carrying the error + un-committed new handle for teardown |
| `LocalClientAdapter` | Typed in-process adapter registering real AppServer connections/channels |
| `JsonRpcAdapter` | Remote adapter: same registration, owns JSON-RPC server-request correlation |
| `ConnectionKey` | Private in-process transport key. Never serialize or persist |
| `AppSessionDataRequest` / `AppSessionDataSource` / `AppSessionDataHandle` | `session/list` / `session/read` / `session/turns/list` composition over persisted-storage callbacks + live registry handles |
| `SurfaceLimits` | Per-connection and per-session passive-surface guards |

## Registry Slots

- `LiveSessionRegistry` stores only lifecycle slots; runtime construction and
  close cascades run in owner tasks calling `complete_load_*` /
  `complete_close`. All three slot states count toward `max_sessions`;
  `get`/`list_live` expose only `Live`. `Closing` keeps the handle for the
  close supervisor, but `begin_load`/resume paths receive only a
  close-completion signal, never the draining handle.
- `spawn_load`: only the caller reserving a fresh `Loading` slot spawns the
  factory future; later callers observe the same completion signal (their
  factories dropped unpolled).
- `spawn_close`: marks `Live -> Closing`, runs the cascade in a spawned task,
  closes routed surfaces, removes the slot. On a `Loading` slot it records
  close-after-load (load failure completes the close signal immediately;
  success moves straight to `Closing` under a single close owner).
- `spawn_shutdown` snapshots every closable slot and starts/observes
  `spawn_close` for each. Higher layers own the process-wide timeout,
  transport stop, hub flush, and exit-code policy.
- `SessionActivityTracker` — lost-wakeup-safe activity clock for lifecycle
  supervisors (updated by load/replace success, attach/detach/disconnect,
  routed events; forgotten on close). Subscribe via `AppServer`, never poll.
- `AppServer::new` uses default `SurfaceLimits`; callers with resolved
  runtime config must use `new_with_surface_limits`.

## Replace / Close Ownership

- `spawn_replace` reserves the replacement as `Loading` (bypassing
  `max_sessions` by exactly one slot), runs construction, commits the
  registry+routing swap, then runs the old handle's close cascade.
  Construction failure removes only the replacement slot. **Commit failure**
  (surface disconnected mid-construction) returns the un-committed new handle
  via `ReplaceCommitFailure<H>`, and the owner task runs `close_handle` on it
  so the runtime's SessionEnd hooks fire and its tasks are joined — **never
  dropped** (that would leak the runtime + tasks for the process lifetime).
- `spawn_replace_to_live` repoints a caller surface to an already-live orphan
  destination (no factory), moves the source to `Closing`, runs its close
  cascade under an `OwnerGuard` in a tracked task. Hosts must route through
  it, **never a bare `tokio::spawn`**.
- `commit_replace_for_surface` takes registry before routing lock and does
  the combined commit in one synchronous section: new `Loading -> Live`, old
  `Live -> Closing`, routing caller old -> new + peer closure.
  (`complete_replace_success` is the registry-only half, no await.)
  `complete_session_close` requires `Closing`, takes registry then routing
  locks, closes routed surfaces, completes the signal, removes the slot.
- Commit methods return `SurfaceLifecycleEffect`s (started/replaced/ended);
  `route_lifecycle_effects` sends them over a separate per-connection
  lifecycle channel **after commit locks are released** — replace emits
  started/replaced before the old close cascade, close emits ended after the
  close commit. Effects can still target `SessionClosed` surfaces because
  connection cleanup metadata survives until disconnect.
- `replace_calling_surface` re-points only the caller; old peers move to
  `SessionClosed`, never auto-attached. `close_session_surfaces` moves every
  live surface to `SessionClosed`, removed from fan-out, connection-side
  cleanup still possible.

## Routing / Lock Order

- Lock order is **registry -> routing -> waiters**; the routing lock is
  always dropped before the `server_request_waiters` mutex is taken.
- Keep the four routing maps in sync: `SurfaceId -> SessionId`,
  `SessionId -> SurfaceId set`, `SurfaceId -> ConnectionKey`,
  `ConnectionKey -> SurfaceId set` — plus `SurfaceAttachment` and
  `interactive_owners`. A second interactive attach returns
  `InteractiveOwnerConflict` with owner metadata (takeover not implemented).
- Session commands carry an explicit typed target — validate against
  connection, surface role, attachment, and live registry entry; never derive
  from a connection-level active-session default.
- `subscribe` must read the retention ring and attach the surface in **one**
  `RoutingState` mutation so replay-to-live has no gap. Only durable
  `SessionEnvelope`s enter the ring; ephemeral envelopes are live-only.
  Honor per-surface `NotificationPrefs` before queueing.
- `attach_live_surface_with_options` / `subscribe_live_surface_with_options`
  validate against the registry (`Live` proceeds, `Closing` ->
  `SessionClosing`, missing/`Loading` -> `SessionNotFound`), holding the
  registry read lock across the routing attach so a concurrent close can't
  orphan the fresh surface. Hosts route `session/subscribe` and interactive
  attach through the `_live_` variants — a bare attach **silently attaches to
  a dead session and hangs the client**.
- Outbound queues are bounded by their transport owner; `RoutingState` uses
  `try_send`, and a full/closed queue disconnects the whole `ConnectionKey`.
  `RouteOutcome`-family results fold that connection's `cancelled_requests`
  into the outcome; AppServer route wrappers call
  `cancel_server_request_waiters` after the routing lock is released.

## Server-Initiated Requests

- Routed only to the interactive surface declaring the required
  `SurfaceCapability`, over a separate per-connection request channel;
  `route_server_request` records pending ownership and `try_send`s. Payloads
  are retained while pending ownership is open so late attach/replay can
  reconstruct actionable requests; complete/cancel removes the payload.
- `resolve_server_request` validates the reply `(connection, surface,
  session, request_id)` against pending ownership and clears pending indexes
  before returning the payload to the runtime/adapter bridge.
- `route_server_request` + `pending_server_request_replays_for_surface` are
  the adapter-facing bridge — adapters must not split delivery/replay
  ownership outside AppServer. Production path for approval, user-input,
  elicitation, MCP-route, and hook callbacks.
- Keep pending indexes in sync by request, session, surface, and turn;
  detach, connection close, turn transition, replace, and session close
  cancel the precise affected ids. `cancel_turn_server_requests(turn_id)` is
  the turn-scoped hook (bridges tag requests with the active turn id via the
  connection's `SessionHandle`).

## Adapters

- `LocalClientAdapter` is typed and in-process only: registers a real
  `ConnectionKey` + event/request/lifecycle channels, then attaches and
  subscribes through the same routing rules as remote adapters.
  `dispatch_client_request` delegates canonical `ClientRequest`s to a
  runtime-supplied `LocalClientRequestHandler` — the local typed seam for
  TUI/headless clients; must not serialize through JSON-RPC.
- `LocalClientConnection::subscribe_surface` returns `SnapshotRequired`
  without a surface id when the replay cursor can't attach; clients
  re-snapshot and resubscribe — never a fake handle. `detach_surface`
  removes only that connection's surface (no session/connection close).
- `JsonRpcAdapter` registers the same channels but keeps wire framing and
  response correlation in JSON-RPC space; delegates decoded `ClientRequest`s
  to a `JsonRpcRequestHandler`; owns no runtime construction or transcript
  parsing. `encode_server_request` records `JsonRpcId -> (SurfaceId,
  RequestId)`; responses resolve through AppServer's typed
  `ServerRequestReply` bridge. `JsonRpcAdapterConnection::app_server`
  exposes the owning `Arc<AppServer<_>>` so bridges stay on the same
  registry/routing instance.
- Connection owner loops exist per transport — NDJSON streams, accepted
  WebSocket streams, caller-supplied frame channels (`run_frame_channels`) —
  plus supervised accept loops for Unix sockets, Windows named pipes, and
  WebSocket listeners (`[bind_and_]run_*_listener_until_shutdown`). All spawn
  one owner task per connection, stop accepting on shutdown, wait for
  accepted owners, and disconnect the AppServer connection on
  EOF/close/transport failure; Unix bind variants own socket-file cleanup via
  the transport listener wrapper. Higher layers own process startup/config
  and shutdown-signal selection.
- Handlers run in an owned task set so a slow lifecycle request can't block
  inbound interrupt/response processing; shutdown aborts and joins them.
  Outbound writes are time-bounded: NDJSON/WebSocket fail the owner with
  `JsonRpcConnectionOwnerError::TransportSlowConsumer`, frame-channel bridges
  with `JsonRpcAdapterError::SlowConsumer`; the owner disconnects the
  connection before returning the error.

## Session Data

- `list_live_sessions`: live-only projection (registry live slots + routing
  surface counts, registry-then-routing lock order).
- `handle_session_data_request` owns `session/list` / `session/read` /
  `session/turns/list` composition over an `AppSessionDataSource`: persisted
  data layered with live registry handles, live-snapshot fallback for
  unpersisted reads. No transcript reads, no `coco-session` dep. Pure
  projection helpers: `parse_session_data_cursor`, `session_data_page`,
  `derive_session_turn_summaries`, etc.

## Deliberate Scope Boundary

This crate owns generic registry, routing, lifecycle coordination, callback
correlation, replay, and pure session-data projection. It deliberately does not
depend on `coco-session` or construct/close application runtimes. Host adapters
supply runtime factories and close cascades to `spawn_load`, `spawn_replace`,
`spawn_close`, and `spawn_shutdown`; interactive takeover and product-specific
listener policy also stay above this crate.
