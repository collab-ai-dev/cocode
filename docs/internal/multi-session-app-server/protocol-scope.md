# AppServer Protocol Scope

Status: normative as of 2026-07-20. This is a breaking protocol. Removed
surface identities and interactive/passive ownership rules have no
compatibility path.

## Core model

The protocol has three independent concepts:

1. A `Connection` is one physical transport plus its immutable initialize
   profile. An NDJSON, WebSocket, Unix socket, or named-pipe peer owns one
   connection. The local TUI owns one in-process AppServer connection; cloned
   local clients and observers are in-memory views over that connection.
2. A `SessionGrant` authorizes one connection for one session. Its access is
   exactly `ReadOnly` or `Full`.
3. A live `SessionAttachment` subscribes a granted connection to ordered
   events. It owns notification preferences and replay cursor state, not
   authorization.

There is no interaction lease, active surface, foreground owner, or unique
writer. Multiple Full connections may mutate the same session. Normal business
coordination determines ordering; server-initiated human responses use the
first valid response.

## Targets and grants

Every session request carries:

```rust
pub struct SessionTarget {
    pub session_id: SessionId,
}
```

Start and resume grant `Full`. Subscribe grants `ReadOnly`. Repeating a
ReadOnly subscribe never downgrades an existing Full grant.

A live attach creates or upgrades the grant and installs the event attachment
atomically with replay. Closing a session removes live attachments but retains
the connection's grants so the same client can read or explicitly delete the
durable record. Disconnect and explicit detach revoke that connection's
grants. Successful delete revokes all grants for the deleted identity.

ReadOnly may read snapshots, turns, status, cost, tasks, config, and ordered
events. It cannot start/interrupt turns, change runtime/config state, close,
replace, delete, or resolve actionable server requests. Full has all session
permissions.

## Request scopes

| Scope | Representative methods | Rule |
|---|---|---|
| Connection | `initialize`, `control/keepAlive`, `control/cancelRequest` | Owned by one transport connection |
| Process | `session/list` | Does not infer an active session |
| Lifecycle | `session/start`, `session/resume`, `session/replace`, `session/subscribe`, `session/close`, `session/delete` | Explicit target where applicable; grant rules below |
| Session read | `session/read`, `session/turns/list`, status/cost/task queries | Requires ReadOnly or Full grant |
| Session full | `turn/start`, `turn/interrupt`, runtime controls, mutations | Requires a live attachment and Full grant |
| Configuration | config read/write | Read requires ReadOnly; write requires Full |

`session/resume` against an already-live identity attaches another Full
connection. It does not require that connection to duplicate another
connection's hook or client-MCP profile.

## Lifecycle

`session/start` mints and constructs a new runtime, promotes it to Live, then
attaches the calling connection with Full access.

`session/resume` loads a missing durable runtime or attaches to an already-live
runtime. Both outcomes return the session identity and Full grant.

`session/replace` requires Full access to the source. The calling connection is
attached Full to the destination; all live attachments to the terminal source
are ended. Source grants remain usable for durable reads until disconnect,
detach, or delete.

`session/subscribe` requires a replay cursor. Replay and live attachment are
one routing mutation. A stale or missing cursor returns `snapshot_required`
and creates neither an attachment nor a grant.

`session/close` requires a live attachment and Full grant. It drains the
runtime, sends terminal lifecycle effects, removes live attachments and the
registry slot, and preserves durable storage and grants.

`session/delete` requires a Full grant for the exact target, rejects every
Loading/Live/Closing slot, removes durable state only, and revokes every grant
for that identity. Close and delete are never combined. While durable deletion
runs the identity is marked deleting, and every slot reservation
(load/resume/replace/child) refuses it, so a concurrent resume cannot publish
a live runtime over rows mid-deletion.

## Events and replay

Every event envelope contains its `session_id`; durable events also carry a
monotonic per-session sequence. Routing uses bounded per-connection queues.
A slow connection is disconnected as a unit and does not block or disconnect
healthy peers.

Retention rings are session-owned and contain durable events only. Subscribe
replay is ordered and gap-free with the subsequent live stream. Close removes
the ring after terminal routing.

## Server-initiated requests

Approval, user input, and ordinary MCP elicitation requests are broadcast to
all currently attached Full connections. ReadOnly connections never receive
them. AppServer registers the waiter before publication, validates reply type,
and atomically accepts the first valid response. Losing clients receive
`control/cancelRequest` and must purge their local correlation.
`input/requestUserInput` is reserved: its wire shape and broadcast semantics
are defined, but no server path currently emits it — AskUserQuestion rides the
approval request.

Pending requests are indexed by request, session, and optional turn. Turn end,
replacement, close, and timeout cancel the whole request, notify recipients,
and resolve the server waiter. A client `control/cancelRequest` withdraws only
that connection from a broadcast; peers retain their opportunity to answer.
The request is cancelled when the last recipient withdraws. An error reply is
not a valid answer: on a broadcast it likewise withdraws only the sender, and
the last recipient's error cancels the request. Error replies complete only
connection-targeted requests — the sole responder failed and the waiter
receives the error.

Disconnect is deliberately weaker than withdrawal: it only prunes the
recipient. A broadcast whose recipients all disconnect stays pending so a
newly attached Full connection receives it via attach-time replay, oldest
first by mint order, bounded by the server request timeout (default 900 s).
This is intentional crash-reconnect rescue; only an explicit cancel or error
reply from the last recipient cancels the request early.

Hook callbacks and client-hosted MCP routing differ because their handler lives
inside a particular client process. Those requests are targeted to the
connection that registered the callback or MCP server. Owners form a
registration stack per `(session, callback)`, most recent registrant first;
routing — including client-MCP elicitation — resolves the current owner per
call, and with no owner falls back to the Full broadcast. Disconnect and
detach prune the connection from every stack, so ownership falls back to a
prior still-attached registrant instead of orphaning the callback. Adding
another Full connection neither blocks attachment nor silently transfers
ownership. A new connection that explicitly registers the same callback/server
becomes the new owner under normal last-registration-wins coordination.
Registration becomes routable only after the session is live and the owning
connection has a Full attachment. Unpublished construction cannot create an
AppServer callback owner or initiate client-hosted MCP traffic.

## Concurrency contract

Session mutations do not use a global writer lease. Two Full clients may issue
commands concurrently. Existing session/runtime invariants decide conflicts:
for example, the turn coordinator admits only one active turn, while independent
configuration writes use their normal atomic update rules.

Human-response races are first-valid-response-wins. Late, duplicate,
wrong-session, wrong-kind, ReadOnly, and non-recipient responses cannot consume
pending state.

## Wire removals

The protocol contains no serialized transport key, UI identity, active-owner
token, history override, or compatibility wrapper. `TurnStartParams` carries
typed prompt/composer/image data plus slash-command metadata and turn-scoped
model / permission-mode / thinking overrides (`goal_continuation` is
`#[serde(skip)]` host-internal and never crosses the wire); history
replacement is an in-process session operation performed before turn
admission.
