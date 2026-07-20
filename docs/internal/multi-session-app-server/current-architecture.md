# Current Multi-Session AppServer Architecture

Verified against the production tree on 2026-07-20.

## Connection model

Each accepted remote transport owns one `ConnectionKey`, immutable
`ConnectionProfile`, bounded event/request/lifecycle senders, and request
correlation. EOF, transport failure, slow-consumer overflow, or owner drop
disconnects the whole connection and resolves its pending work.

The TUI creates one `LocalClientConnection`. `LocalServerClient::clone`,
session clients, command handles, and observers reuse that key and communicate
through a bounded in-memory demultiplexer. They are not additional physical
connections.

## Grants and attachments

`RoutingState` stores grants separately from live attachments:

- `SessionGrant` is `(connection, session, ReadOnly | Full)` authorization;
- `SessionAttachment` is live event delivery preferences and cursor state;
- active attachment indexes drive event and server-request fan-out.

Start/resume attach Full; subscribe attaches ReadOnly. A later ReadOnly attach
cannot downgrade Full. Close removes attachments and replay state but retains
grants for durable read/delete. Disconnect or explicit detach revokes that
connection's grants; delete revokes every grant for the identity.

Remote `session/resume` can attach another Full connection to a live runtime.
There is no callback-profile compatibility gate and no unique writer.

## Lifecycle ownership

`LiveSessionRegistry` owns Loading, Live, and Closing slots. `AppServer`
coordinates registry and routing commits under registry-before-routing lock
order. Runtime construction and close cascades execute in tracked owner tasks,
so dropping a request caller cannot strand lifecycle state.

`session/close` validates a live Full target, drains the runtime under one
deadline, commits terminal routing/lifecycle effects, and preserves durable
storage. `session/delete` requires the target's retained Full grant, rejects a
live/loading/closing slot, deletes storage, and revokes grants.

## Events and local demultiplexing

Every event is session-keyed. Durable events receive a monotonic sequence and
enter the session retention ring. Routing uses `try_send` on bounded
per-connection queues; a slow connection is disconnected without affecting
peers.

The local connection starts draining immediately. Session events enter one
bounded in-memory broadcast ring and every local client view owns an independent
cursor, so the TUI event pump cannot steal `TurnEnded` from a turn waiter.
Server requests and lifecycle effects use separate bounded dispatcher queues.

## Server requests

AppServer prepares pending state, installs the response waiter, then publishes.
Approval, user input, and normal elicitation broadcast to all attached Full
connections. Reply validation includes connection, grant, recipient, session,
request id, and reply variant. The first valid reply wins; every loser receives
`control/cancelRequest`.

Timeout, turn end, close, and replacement all remove the same pending indexes,
notify recipients, and close the waiter. Client cancellation removes only that
recipient from a broadcast; the waiter remains for peers until a reply, a
system cancellation, or the last recipient's withdrawal. JSON-RPC, Python, and
TypeScript routers purge local correlation on server cancellation.

Hook callbacks and client-hosted MCP messages are connection-owned. The route
looks up the current owner at invocation time. Adding a Full Web/TUI peer does
not transfer ownership; explicit registration of the same id/name does.
Runtime callback definitions are installed during unpublished construction,
but AppServer ownership, SessionStart hook execution, and client-MCP connection
begin only after the runtime is live and the Full connection is attached.

## TUI permission flow

TUI approvals use the same AppServer request path as every other Full client.
The TUI presentation adapter consumes `AskForApproval` from its existing local
connection and resolves it through AppServer. A Web Full peer may answer the
same request first; AppServer cancels the TUI task and clears its pending UI
entry. There is no leader bridge or interaction lease.

## Turn input

All entry paths call the shared mention resolver per turn. The resolver does
not hold `FileReadState` locks across I/O, applies read/sandbox permission
checks in production, moves blocking generation/listing off the async runtime,
and bounds count, size, token budget, directory listing, and I/O duration.

Public `turn/start` contains typed prompt, composer, images, and optional model
controls. History replacement is a typed in-process session operation; the
wire has no history override.

## Evidence

Focused regression suites cover:

- second-turn file mention after a plain first turn;
- one physical local connection across clones and consecutive event bursts;
- multiple Full controllers plus ReadOnly denial;
- grant persistence across close and revocation on delete/disconnect;
- live Full resume without callback-profile coupling;
- connection-owned callback/MCP routing;
- SessionStart client hooks route only after live Full attachment;
- waiter publication race, wrong reply kind, timeout, cancellation, and first
  response wins;
- replay gaps, queue overflow, lifecycle, close/delete, and SDK router
  cancellation behavior.

The normative rules are in [target-architecture.md](target-architecture.md)
and [protocol-scope.md](protocol-scope.md). Review, history, and remediation
documents record superseded designs and must not be used as current contracts.
