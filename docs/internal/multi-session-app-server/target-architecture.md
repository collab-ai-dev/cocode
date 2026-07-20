# Multi-Session AppServer Target Architecture

Status: normative as of 2026-07-20.

## Goals

- One process hosts zero or more independent sessions.
- One physical transport maps to one AppServer connection.
- A connection may hold ReadOnly or Full grants for many sessions.
- Many Full connections may concurrently control the same session.
- Authorization, event subscription, transport, runtime, and callback routing
  have separate owners.
- TUI, headless, SDK, WebSocket, and future Web UI clients use the same typed
  lifecycle and request semantics.
- There is no compatibility layer for removed UI-owner concepts.

## Ownership

```text
physical transport
  -> ConnectionKey + immutable ConnectionProfile
       -> SessionGrant(session, ReadOnly | Full)
       -> connection-owned hook/MCP callbacks
       -> bounded event/request/lifecycle channels

live session
  -> registry slot (Loading | Live | Closing)
  -> SessionHandle runtime
  -> zero or more live SessionAttachments
  -> durable replay ring
  -> pending server-request broker
```

`ConnectionKey` is private process state and is never serialized. A local TUI
uses one `LocalClientConnection`; clones, command handles, turn waiters, and
observers share a bounded in-memory demultiplexer. Every remote WebSocket or
stream is a separate physical connection.

`SessionGrant` contains authorization only. It survives runtime close for
durable reads and deletion, but is removed on disconnect, explicit detach, or
successful delete.

`SessionAttachment` contains notification preferences, delivery cursor, and
attachment time only. It exists while that connection consumes a live session.
Close removes attachments without conflating them with grants.

## Crate boundaries

```text
coco-types
  protocol DTOs, SessionAccess, event/request delivery types

coco-app-server
  registry, grants, attachments, replay, pending request broker,
  local/JSON-RPC adapter state

coco-app-server-client
  remote typed client and bounded demultiplexer

coco-agent-host
  runtime construction, lifecycle handler, local typed facade,
  connection-owned hook/MCP registration

coco-sdk-server
  SDK transport binding only

coco-cli
  TUI/headless/SDK composition and terminal presentation
```

The server crate never constructs application runtimes or reads transcripts.
The client crate never depends on the server implementation. Agent-host does
not own terminal UI state.

## Locking and state transitions

Cross-registry operations use the canonical order:

```text
registry -> routing -> server-request waiters
```

No await occurs while these synchronous locks are held. Load, replace, close,
and shutdown use tracked owner tasks. Caller cancellation cannot strand a
Loading or Closing slot.

Start and resume construction finish before a runtime is promoted and attached.
Construction may install runtime-local callback definitions, but it must not
publish connection ownership, invoke client callbacks, or connect client-hosted
MCP until promotion and Full attachment succeed. Failed factories therefore
leave no ghost callback owner.
Replacement commits destination promotion, source Closing transition, and
routing changes in one no-await section. Close drains owned work, commits the
terminal state, routes lifecycle effects after locks are released, removes the
slot and replay ring, and leaves no live attachment.

## Concurrent Full clients

There is deliberately no interaction lease or single writer. Full clients may
start turns, interrupt, update configuration, close, or replace subject to the
runtime operation's own concurrency rules. The turn coordinator remains the
single source of truth for one-active-turn admission; it returns a normal busy
conflict instead of assigning a privileged client.

Connection limits are independent capacity controls: a connection may attach
to a bounded number of sessions and a session may accept a bounded number of
connections. Reaching a limit rejects the new attachment; it never changes
grant semantics or chooses a writer.

A second connection obtains Full access by resuming the live identity. Its
initialize profile need not reproduce hooks or client MCP servers owned by
another connection.

## Callback ownership

Human interactions that any user-facing Full client can answer are broadcast:
approval, user input, and ordinary elicitation. First valid response wins.

Hooks and client-hosted MCP servers execute in a client process, so their
registration records `(session, callback) -> connection`. Routing looks up the
owner for every invocation; it does not capture a stale connection in a global
session closure. Disconnect clears that connection's registrations. Explicit
re-registration may transfer ownership.

## Pending request broker

For each server request AppServer stores one entry containing payload,
audience, eligible recipients, session, optional turn, and monotonic creation
order. Publication happens only after the response waiter is installed.

Completion validates:

- request exists;
- response session matches;
- connection has Full access and was a recipient;
- response variant matches the request;
- no previous response won.

Completion removes every index atomically and sends cancellation notifications
to losers. Timeout, turn end, close, and replacement use the same global cleanup
path. A client-originated cancel withdraws only that recipient from a broadcast;
the request and waiter remain live for peers, and are removed only when the last
recipient withdraws. Client routers abort local handlers and remove correlation
on server-originated `control/cancelRequest`.

## Event routing

Events are explicitly session-keyed. Durable events receive a strictly
monotonic sequence and enter a bounded retention ring. Live routing filters by
attachment preferences and uses bounded queues. A full or closed queue
disconnects that connection; it never creates unbounded per-clone fan-out.

The local client drains its one physical inbound stream from connection
creation. Events enter a bounded broadcast ring with one cursor per in-memory
client view; server requests and lifecycle effects enter separate bounded
dispatcher queues. This prevents the TUI event pump and turn waiters from
stealing one another's terminal events while preserving one physical
connection.

## Turn input and history

Every turn resolves mentions independently. File-read dedup state is shared by
the session behind an `Arc<RwLock<_>>`, but no lock guard crosses file I/O.
Mention count, aggregate tokens, file size, directory entries, and blocking I/O
time are bounded. Production mention resolution performs sandbox/read
permission checks before loading content.

History never crosses the public wire as an override. Local resume/switch paths
replace typed session history before starting a turn. An empty turn prompt does
not append a synthetic empty user message.

## Completion criteria

The architecture is complete only while tests demonstrate:

- a TUI client clone creates no second AppServer connection;
- a plain first turn followed by a file mention resolves on the second turn;
- two Full connections can control one session;
- a callback-free Full peer can resume a callback-owning live session without
  stealing callback ownership;
- a SessionStart client hook is delivered after the initiating Full attachment;
- ReadOnly cannot mutate or delete, including after close;
- close preserves grants and durable history, while delete revokes grants;
- immediate responses cannot beat waiter registration;
- first response wins and losers receive cancellation;
- timeout/turn/close cleanup leaves no pending correlation;
- replay is ordered, bounded, and session-isolated;
- Rust workspace checks, Clippy, generated SDK checks, and Python/TypeScript
  router tests pass.
