# Multi-Session AppServer Architecture

Status: current contract verified against the 2026-07-20 refactor. Backward
compatibility with removed UI-owner protocol concepts is not supported.

## Current design

- A process hosts zero or more independent sessions.
- One physical transport owns one connection. The local TUI owns exactly one
  AppServer connection; its clones and observers are bounded in-memory views.
- A connection holds per-session `ReadOnly` or `Full` grants.
- Grants are authorization. Live attachments are event subscriptions. They are
  stored and retired independently.
- Multiple Full connections may concurrently control one session.
- Approval, user input, and ordinary elicitation broadcast to all Full clients;
  the first valid reply wins.
- Hook callbacks and client-hosted MCP routes are connection-owned and targeted.
- Close removes the runtime and live attachments but preserves durable data and
  grants. Delete requires the target's Full grant, deletes durable state, and
  revokes grants.
- Public turn requests contain typed turn input only. History replacement is an
  in-process session operation.
- Mention resolution is per turn, permission-aware, bounded, and performs no
  file I/O while holding session read-state locks.

## Dependency graph

```text
L0 coco-app-server-transport
L1 coco-app-server | coco-app-server-client
L2 coco-app-runtime, coco-query, coco-session, coco-tui
L3 coco-agent-host
L4 coco-sdk-server
L5 coco-cli
```

The remote client never depends on the server implementation. AppServer never
constructs product runtimes or reads transcripts. Agent-host owns runtime
construction and typed request handling. CLI owns TUI/headless composition.

## Documents

| Document | Role |
|---|---|
| [target-architecture.md](target-architecture.md) | Normative ownership, concurrency, routing, and crate boundaries |
| [protocol-scope.md](protocol-scope.md) | Normative request scopes, grants, lifecycle, and wire semantics |
| [current-architecture.md](current-architecture.md) | Descriptive map from the contract to the production tree and tests |
| [review.md](review.md) | Historical 2026-07-13 adversarial review; retains superseded terminology |
| [remediation-plan.md](remediation-plan.md) | Historical implementation record |
| [follow-up-todo.md](follow-up-todo.md) | Historical v2 follow-up record |
| [history.md](history.md) | Superseded decisions and migration history |

Crate-local rules remain authoritative for implementation details:

- `coco-rs/app/server/CLAUDE.md`
- `coco-rs/app/server-client/CLAUDE.md`
- `coco-rs/app/agent-host/CLAUDE.md`
- `coco-rs/app/tui/CLAUDE.md`

## Required evidence

Changes to this architecture must keep regressions for:

- one physical local connection across cloned clients;
- second-turn file mention resolution;
- multiple Full controllers and ReadOnly denial;
- live Full resume without callback-profile coupling;
- SessionStart callback routing after live Full attachment;
- grant persistence across close and revocation on delete/disconnect;
- first-response-wins plus loser cancellation;
- waiter-before-publish ordering and bounded timeout cleanup;
- ordered replay, session isolation, and slow-consumer disconnect;
- Python and TypeScript server-request cancellation/correlation;
- error-reply withdrawal semantics (broadcast withdraw vs targeted completion);
- idle-close abort when a connection is attached;
- delete-guard refusal of slot reservations during durable deletion.
