# Migration History

This document is historical. It records how the design evolved and which
decisions remain valid. It is not normative; see `target-architecture.md` and
`protocol-scope.md`.

## Superseded early designs

### Thread plus session identity

The earliest concurrent-server proposal copied a separate root `ThreadId` and
`SessionId` hierarchy. It was rejected because coco-rs already uses
`SessionId` as the durable root conversation and transcript identity.
Subagents live below that root through `AgentId`; a second root id added
conversion and ownership ambiguity without solving a product requirement.

This rejection remains valid.

### Monolithic multi-session plan

The later plan mixed requirements, speculative types, reference-product
research, migration status, adversarial findings, and future features in one
document. As code changed, its "current" sections contradicted its later
landing notes. It was replaced by the review/current/target/protocol/plan split
in this directory.

The 2026-07-13 audit found that the replacement documents again mixed landed
status with stale future text. V2 keeps the document split but reopens the
architecture and removes claims that test counts alone prove completion.

### Speculative surface and runtime crates

An early V2 draft prescribed three new crates: `coco-agent-runtime`,
`coco-tui-runner`, and `coco-headless`. That prescription was rejected after
comparing it with the current ownership boundaries:

- extracting `SessionRuntime` would mostly copy or mechanically move
  `coco-agent-host` internals while lifecycle and protocol orchestration still
  have only one real owner and consumer;
- TUI and headless are executable surface policy, not independently reusable
  libraries;
- another `surfaces/` directory would add taxonomy without an ownership
  boundary;
- `coco-sdk-server` already has a real reusable boundary around transports,
  ordered writing, and the AppServer JSON-RPC adapter.

V2 therefore refactors `coco-agent-host` in place and uses direct
`app/cli/src/{tui,headless,sdk}` directories. The SDK CLI runner moves there,
while the `coco-sdk-server` transport crate remains separate. A future runtime
crate extraction requires a demonstrated second consumer or a measurable
dependency/compile-time boundary; it must never duplicate agent-host logic.

## V1 work retained

The following implementation work remains part of V2:

1. `SessionId` is the only root conversation identity.
2. `SessionEnvelope` carries session/agent/turn attribution and per-session
   durable sequencing.
3. AppServer owns bounded replay and per-surface fan-out.
4. `LiveSessionRegistry` uses Loading/Live/Closing slots and spawned owner tasks
   so caller cancellation does not own lifecycle progress.
5. registry/routing commits use a fixed no-await lock order.
6. interactive and passive surfaces are distinct, with at most one interactive
   owner per session.
7. `coco-app-server-client` does not depend on the server implementation.
8. in-process local client composition lives above the generic server.
9. live backend/tool/TUI state have separate owners; there is no useful global
   `AppState`.
10. process/project bootstrap contracts remain separate from fused application
    session construction.
11. every interactive request carries explicit target authority.
12. accepted remote connections own independent initialize profiles and writer
    correlation.
13. turn execution receives the AppServer-selected session capability.
14. turn/MCP/file-history/reload/callback state is session-keyed in the tested
    production paths.
15. closing resume, explicit replacement, orphan authority, slow-consumer
    recovery, and multi-runtime shutdown tests remain valuable.

## V1 decisions still rejected

The following remain rejected:

| Idea | Reason |
|---|---|
| Whole `SessionRuntime` actor and universal mailbox | Creates a god actor and unnecessary request/reply plumbing for independent reads/services |
| Mutable process current-session slot | Makes multi-session selection implicit and unsafe |
| `ProjectHeavyServices` aggregate | Cost is not a responsibility or lifecycle contract |
| Sharing mutable services merely to reduce object count | Sharing requires an explicit key, isolation model, refresh, and teardown contract |
| Optional/missing request target meaning current session | Makes authority depend on runtime context |
| Product UI logic inside generic AppServer | Reverses dependency direction |

## V1 breaking refactor (2026-07-11)

The July 11 refactor:

- added typed targets and exhaustive request scopes;
- isolated connection initialize/profile/callback state;
- made AppServer validation the ordinary interactive runtime selector;
- moved active turns and integrations behind session handles;
- added explicit replacement, closing retry, orphan authority, and concurrent
  shutdown;
- removed earlier singleton runtime/capability slots;
- added production A/B authority and isolation tests;
- enabled `clippy::await_holding_lock` workspace-wide.

Those changes addressed real cross-session mixing risks and should not be
rolled back.

## Why V1 completion was reopened (2026-07-13)

An adversarial review cross-validated the architecture documents against the
current production tree and tests. It found:

1. `session/archive` deletes JSONL despite documentation that close preserves
   it.
2. archive snapshots accounting before turn drain and can detach timed-out
   turn/forwarder tasks, allowing late work after close.
3. CLI mode flags are accepted but do not select their documented runners.
4. SDK startup constructs a hidden session before initialize/start/resume.
5. Event Hub announces that hidden identity as a static one-session live set.
6. TUI/headless startup resume bypasses the formal lifecycle used by SDK.
7. agent-host depends on TUI and exports most implementation modules.
8. `SessionHandle` hides the runtime type but exposes raw mutable resources
   through a very broad forwarding API.
9. session identity and callback requirements are not fully enforced by
   construction.
10. existing integration tests do not cover these properties.

A follow-up cross-layer review found additional correctness defects:

11. serialized `session/start` can name an existing live/orphan session and
    mutate it before interactive ownership is established;
12. turn coordination returns to Idle and emits terminal events before final
    engine history is committed, allowing an immediate next turn to read stale
    history;
13. multiple accepted initialize/start fields are silently ignored, and
    start/resume response surface ids remain optional compatibility paths;
14. close and process shutdown do not prove that every session-owned task is
    cancelled, aborted, and joined under one deadline;
15. one replace branch detaches old-session close, while another reports
    success before old close completion/failure is known;
16. Event Hub membership excludes Closing sessions before their final result
    is durably handed to egress, so reconnect cursor state can omit that event.

The audit therefore changed the status from "complete" to "v2 remediation
required". It did not invalidate the v1 explicit-target or registry work.

## V2 breaking decisions

V2 adopts these new decisions:

1. remove `session/archive`;
2. add runtime-only `session/close` and storage-only `session/delete`;
3. make the registry close owner responsible for all task teardown and terminal
   result ordering;
4. introduce one typed CLI `ExecutionPlan` and delete unsupported flags/commands;
5. make process host startup create zero sessions;
6. derive Event Hub live membership from the AppServer registry;
7. require all surfaces to use the same typed lifecycle client;
8. place executable surface composition directly under
   `app/cli/src/{tui,headless,sdk}` without another directory or new runner
   crates;
9. retain `coco-sdk-server` as the transport/connection adapter crate and move
   only CLI SDK startup policy into `app/cli/src/sdk`;
10. refactor `coco-agent-host` in place; a separate agent-runtime crate is not
    part of this refactor;
11. remove TUI dependencies from agent-host;
12. make session identity and callback requirements construction-time data;
13. expose operations/snapshots rather than raw locks and managers;
14. organize modules by lifecycle, operation, and integration ownership;
15. remote start mints a new identity, accepts no initial history, and cannot
    enter an existing registry slot;
16. every accepted protocol field is consumed or rejected explicitly;
17. one owner commits history/accounting, joins tasks, and only then emits
    terminal lifecycle events;
18. replace and Hub retirement expose post-commit failure/final-egress state
    instead of reporting early success.

Implementation status is tracked only in `remediation-plan.md` until these
decisions land.

## V2 convergence reset (2026-07-13)

After multiple implementation/review rounds, V2 stopped treating every valid
adjacent finding as part of one active refactor. The warning signs were:

- a new Phase 0 correctness hole was found after Phases A-F had partially
  landed;
- locally useful lifecycle changes widened the remote protocol;
- component suites passed while cross-owner completion races remained;
- proposed runtime/surface crates were withdrawn after their ownership seam was
  re-evaluated;
- normative architecture, implementation status, and cleanup scope kept
  changing together.

The corrective decision is a serial three-workstream cadence:

1. correctness stabilization closes only CS-1 start, CS-2 turn, CS-3
   close/replace/tasks, and CS-4 Hub retirement;
2. surface boundary then moves composition to
   `app/cli/src/{tui,headless,sdk}` and removes host -> TUI without behavior
   changes;
3. internal cleanup then narrows/reorganizes agent-host and removes obsolete
   code without protocol changes.

Existing Phase letters remain migration history, while the workstream order in
`remediation-plan.md` is normative. Each correctness item begins with a failing
adversarial regression and closes one named owner/completion contract. New
cleanup, platform coverage, or architectural ideas are queued outside the
active gate unless they prove one of CS-1 through CS-4 impossible.
