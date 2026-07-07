# coco-app-server-client

Typed client-side handles for AppServer. This crate is currently a Phase A
foundation slice: it exposes an in-process `ServerClient` over
`coco_app_server::LocalClientAdapter` plus typed interactive/passive surface
handles. Remote transports and JSON-RPC are not implemented here yet.

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
  `try_next_session_event` / `try_next_passive_event` demux the shared event
  receiver by `SurfaceId` and buffer other surfaces; the full async stream API
  lands with the transport/client work.
- Server-request and lifecycle receivers follow the same `SurfaceId` demux
  rule. Reading one handle's request/lifecycle queue must not consume another
  surface's delivery on the same connection.
- `ServerClient::list_live_sessions` is the client-side live projection for
  future `list_sessions`: it returns `SessionId` plus current surface counts,
  not persisted transcript metadata.
- `ServerClient::detach_passive` consumes a passive handle and removes only
  that surface. It does not close the connection or archive the session.

## Pending

Persisted session listing, `ConnectOptions`, remote transports, JSON-RPC
request/response correlation, stream adapters, start/resume runtime creation,
query/interrupt/replace/close session operations, disconnect invalidation, and
typed public client errors for every server failure remain pending Phase A
work.
