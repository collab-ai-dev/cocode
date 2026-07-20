# coco-app-server

App-server ownership and routing layer: connection/session routing, replay
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
| `RoutingState` | Single-lock state for grants, live attachments, durable rings, and pending requests |
| `SessionGrant`, `SessionAccess` | Per-connection authorization for one session (`ReadOnly` or `Full`); retained across close |
| `SessionAttachment` | Live event preferences and delivery cursor; contains no authorization |
| `AttachError` | Snafu-backed live-attach failure (`SessionClosing`, `SessionNotFound`) |
| `ReplaceCommitFailure<H>` | Replacement commit failure carrying the error + uncommitted new handle for teardown |
| `LocalClientAdapter` | Typed in-process adapter registering real AppServer connections/channels |
| `JsonRpcAdapter` | Remote adapter: same registration, owns JSON-RPC server-request correlation |
| `ConnectionKey` | Private in-process transport key. Never serialize or persist |
| `AppSessionDataRequest` / `AppSessionDataSource` / `AppSessionDataHandle` | `session/list` / `session/read` / `session/turns/list` composition over persisted-storage callbacks + live registry handles |

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
  closes routed attachments, removes the slot. On a `Loading` slot it records
  close-after-load (load failure completes the close signal immediately;
  success moves straight to `Closing` under a single close owner).
- `spawn_shutdown` snapshots every closable slot and starts/observes
  `spawn_close` for each. Higher layers own the process-wide timeout,
  transport stop, hub flush, and exit-code policy.
- `SessionActivityTracker` — lost-wakeup-safe activity clock for lifecycle
  supervisors (updated by load/replace success, attach/detach/disconnect,
  routed events; forgotten on close). Subscribe via `AppServer`, never poll.

## Replace / Close Ownership

- `spawn_replace` reserves the replacement as `Loading` (bypassing
  `max_sessions` by exactly one slot), runs construction, commits the
  registry+routing swap, then runs the old handle's close cascade.
  Construction failure removes only the replacement slot. **Commit failure**
  (connection disconnected mid-construction) returns the uncommitted new handle
  via `ReplaceCommitFailure<H>`, and the owner task runs `close_handle` on it
  so the runtime's SessionEnd hooks fire and its tasks are joined — **never
  dropped** (that would leak the runtime + tasks for the process lifetime).
- `spawn_replace_to_live` repoints a caller connection to an already-live
  destination (no factory), moves the source to `Closing`, runs its close
  cascade under an `OwnerGuard` in a tracked task. Hosts must route through
  it, **never a bare `tokio::spawn`**.
- `commit_replace_for_connection` takes registry before routing lock and does
  the combined commit in one synchronous section: new `Loading -> Live`, old
  `Live -> Closing`, routing caller old -> new + detaching old-session peers.
  (`complete_replace_success` is the registry-only half, no await.)
  `complete_session_close` requires `Closing`, takes registry then routing
  locks, closes routed attachments, completes the signal, removes the slot.
- Commit methods return `SessionLifecycleEffect`s (started/replaced/ended);
  `route_lifecycle_effects` sends them over a separate per-connection
  lifecycle channel **after commit locks are released** — replace emits
  started/replaced before the old close cascade, close emits ended after the
  close commit.
- Replacement grants/attaches the calling connection with `Full` access to the
  new session and detaches every connection from the terminal old session.
  Close removes live attachments while retaining grants for durable read and
  explicit delete.

## Routing / Lock Order

- Lock order is **registry -> routing -> waiters** — always acquire in that
  direction; no path may take a lock earlier in the order while holding a
  later one. Most wrappers drop the routing lock before touching the
  `server_request_waiters` mutex, but `route_server_request_with_reply*`
  deliberately holds the routing write lock across the waiter insert and the
  publish: that continuous hold is what makes waiter-before-publish atomic
  and keeps the prepared pending entry alive for publication.
- Internal slow-consumer disconnects (inside route/publish/notify) record the
  removed connection-targeted request ids in `RoutingState`'s orphaned-waiter
  buffer; every `AppServer` wrapper that takes the routing write lock drains
  it after unlock via `cancel_server_request_waiters`. Never leave that
  buffer undrained — a stranded reply waiter blocks its hook/MCP bridge until
  the request timeout.
- Keep the two live-routing indexes and attachment map in sync:
  `SessionId -> ConnectionKey set`, `ConnectionKey -> SessionId set`, and
  `(ConnectionKey, SessionId) -> SessionAttachment`.
- Grants live in a separate `(ConnectionKey, SessionId) -> SessionGrant` map.
  Disconnect/detach revoke them; close retains them; successful delete revokes
  every grant for the deleted identity.
- Capacity limits are resource policy only: one connection has a bounded number
  of attached sessions and one session has a bounded number of connections.
  They never elect an owner or serialize Full mutations.
- Session commands carry an explicit typed target — validate against
  the connection's session grant and live registry entry; never derive
  from a connection-level active-session default.
- Connection-owned callback registration requires a live Full attachment.
  A retained post-close grant is not sufficient, and unpublished runtime
  construction must not create callback ownership.
- `subscribe` must read the retention ring and attach the connection in **one**
  `RoutingState` mutation so replay-to-live has no gap. Only durable
  `SessionEnvelope`s enter the ring; ephemeral envelopes are live-only.
  Honor per-attachment `NotificationPrefs` before queueing. A read-only
  subscribe must never downgrade an existing Full grant.
- `attach_live_session` / `subscribe_live_session`
  validate against the registry (`Live` proceeds, `Closing` ->
  `SessionClosing`, missing/`Loading` -> `SessionNotFound`), holding the
  registry read lock across the routing attach so a concurrent close cannot
  create a dead attachment. Hosts route lifecycle operations through these
  live-checked variants.
- Outbound queues are bounded by their transport owner; `RoutingState` uses
  `try_send`, and a full/closed queue disconnects the whole `ConnectionKey`.
  Such an internal disconnect records the connection's cancelled
  connection-targeted request ids in the orphaned-waiter buffer, and every
  AppServer routing-write wrapper drains it into
  `cancel_server_request_waiters` after the lock is released (see Routing /
  Lock Order above).

## Server-Initiated Requests

- Broadcast to every Full connection for the session over the separate
  per-connection request channel. ReadOnly connections never receive
  actionable requests.
- `resolve_server_request` validates `(connection, session, request_id)` and
  atomically removes the pending entry. Therefore concurrent valid responders
  use first-response-wins semantics; later replies receive NotFound.
- An **error reply is not a valid answer**: on a broadcast it withdraws only
  the sender (last-recipient error cancels the request); it completes only a
  connection-targeted request, whose sole responder failed
  (`ErrorReplyDisposition` / `ServerRequestResolution`).
- Callback owners are a per-`(session, callback)` **registration stack**,
  most recent first. Routing targets the front entry; disconnect/detach prune
  the connection from every stack so ownership falls back to a prior
  still-attached registrant instead of orphaning the callback.
- Keep pending indexes in sync by request, session, and turn. Turn transition,
  replace, and session close cancel the precise affected ids; disconnecting one
  Full connection does not invalidate requests still answerable by peers.
  `cancel_turn_server_requests(turn_id)` is
  the turn-scoped hook (bridges tag requests with the active turn id via the
  connection's `SessionHandle`).
- Install response waiters before publication. Completion, timeout, turn end,
  replace, and close purge the same indexes and notify aborted/losing
  recipients with `control/cancelRequest`. A client cancellation withdraws
  only that recipient from a broadcast; the waiter is cancelled only when its
  last eligible recipient withdraws.
- The request timeout is configured by the host (15 minutes by default), so a
  vanished responder cannot leak a waiter while normal human input is not
  constrained to a one-minute window.

## Adapters

- `LocalClientAdapter` is typed and in-process only: registers a real
  `ConnectionKey` + event/request/lifecycle channels, then attaches and
  subscribes through the same routing rules as remote adapters.
  `dispatch_client_request` delegates canonical `ClientRequest`s to a
  runtime-supplied `LocalClientRequestHandler` — the local typed seam for
  TUI/headless clients; must not serialize through JSON-RPC. One local client
  instance owns one AppServer connection; client clones are in-memory views.
- `LocalClientHandle::subscribe_session` returns `SnapshotRequired` when the
  replay cursor is unavailable; clients re-snapshot and resubscribe.
  `detach_session` removes only that connection's attachment and grant.
- `JsonRpcAdapter` registers the same channels but keeps wire framing and
  response correlation in JSON-RPC space; delegates decoded `ClientRequest`s
  to a `JsonRpcRequestHandler`; owns no runtime construction or transcript
  parsing. `encode_server_request` records `JsonRpcId -> (SessionId,
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
  Full/ReadOnly connection counts, registry-then-routing lock order).
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
`spawn_close`, and `spawn_shutdown`; product-specific listener policy stays
above this crate.
