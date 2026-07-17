# Round 1 — Requirements Analysis

> Goal of this round: build a shared, well-grounded picture of (a) what
> `codex-rs` actually solved by introducing storage, (b) where `coco-rs`
> stands today on the same axes, (c) what the user's scenario really
> demands, and (d) the open questions we must answer before any code is
> written. **No design decisions are locked in this document.**

---

## 1. What problem did `codex-rs` solve by introducing storage?

`codex-rs` is not "an agent with a DB tacked on". Storage was introduced
to solve **five separable problems**, each with a different crate:

### 1.1 Replay / resume — `codex-rs/rollout/`

- **Format**: append-only JSONL files, one per session
  (`~/.codex/sessions/rollout-<ts>-<uuid>.jsonl`).
- **Why JSONL, not SQLite**: every `RolloutItem` is durable on `fsync`
  (POSIX `O_APPEND` + line semantics), and the file is the **canonical**
  history. Resume = replay the JSONL. SQLite is not required to replay.
- **Recorder is async + batched** (`RolloutRecorder` with an mpsc channel
  to a single writer task), so the hot path never blocks on disk.
- **Persistence policy** (`policy.rs`): two modes, `Limited` (minimal
  replay surface) and `Extended` (richer event surface for an
  app-server-driven history view). The wire variants `RolloutItem` carries
  are deliberately a **subset** of internal events — the JSONL is not the
  full event firehose.

### 1.2 Cross-session metadata index — `codex-rs/state/`

- **Format**: SQLite (`state_5.sqlite` in `~/.codex/sqlite/`).
- **Role**: not the source of truth — a **rebuildable index** extracted
  from JSONL. `apply_rollout_item()` folds new JSONL records into rows,
  `BackfillStatus` tracks reconstruction state.
- **Why it exists**: listing / sorting / filtering thousands of sessions
  is intractable if you must `tail`-walk thousands of JSONL files. Schema
  is queryable metadata (timestamps, model, status, parent edges), never
  the full event payload.
- **Twin DB**: `logs_2.sqlite` for backfill-process state itself (so
  recovery is itself recoverable).

### 1.3 Storage-neutral abstraction — `codex-rs/thread-store/`

- A trait (`ThreadStore`) that hides which backend is in use. Two impls:
  `LocalThreadStore` (rollout JSONL + `state` SQLite) and
  `InMemoryThreadStore` (tests).
- **`LiveThread`** is the per-session handle the runtime holds; it routes
  appends to the store, owns the metadata patch helper, and applies the
  rollout-persistence policy. Producers don't talk to JSONL or SQLite —
  they talk to `LiveThread`.
- **Explicit metadata API**: appends do not auto-infer metadata; metadata
  patches are a separate `update_thread_metadata` call. This avoids the
  "we accidentally re-derived state from the firehose" footgun.

### 1.4 Multi-agent topology — `codex-rs/agent-graph-store/`

- Parent/child edges between threads (multi-agent v2). Storage layered on
  top of `state`'s SQLite — not a separate DB. Tracks `Open / Closed`
  status per edge so you can walk the spawn tree.
- **Why separate from `state`**: graph queries (descendants, children
  list) are a distinct API surface from "list threads by recency". One
  schema, two stores.

### 1.5 Diagnostic forensics — `codex-rs/rollout-trace/` (opt-in)

- Completely separate from the product history. Opt-in via
  `CODEX_ROLLOUT_TRACE_ROOT`. Local bundle: `manifest.json`,
  `trace.jsonl`, `payloads/*.json`, optional `state.json` from
  `codex debug trace-reduce`.
- Purpose: reproduce bugs from raw evidence (full request/response
  payloads, tool outputs, terminal captures). Not telemetry — stays
  local, never uploaded.
- **Architectural note**: shared writer across an entire agent spawn
  tree, so multi-agent runs produce one semantic graph.

### 1.6 Cross-process API — `codex-rs/app-server/`

- HTTP/RPC server (`axum`-style) that exposes `ThreadStore` over the
  wire: list threads, read a thread's history, patch metadata. The
  consumers are IDEs, debug clients, and the codex desktop app — they
  do not link to the runtime; they call the server.
- **`app-server-protocol`** owns the wire types. The split mirrors what
  we'd want for an event hub: a producer-agnostic protocol crate plus a
  thin transport adaptor.

### 1.7 What is *not* in `codex-rs`

Worth calling out because it bounds the reference:

- **No web UI in the repo.** The viewer is in the calling product
  (Claude desktop app, IDE plugins). The `debug-client` crate is a CLI.
- **No central pub/sub bus.** Consumers pull from the HTTP API, they
  don't subscribe to a stream.
- **No remote ingest.** Storage is local-first; `app-server` serves
  locally-stored sessions, it doesn't aggregate remote agents.

So `codex-rs` solves the **single-host persistence + local API** problem
very well. It does **not** solve "many agents stream to one place" — that
is exactly what we are adding to `coco-rs`.

---

## 2. Where `coco-rs` stands today

### 2.1 Event taxonomy (already strong)

`coco-rs` has a deliberately rich event model (`coco-types::CoreEvent`)
with three layers:

| Layer | What it carries | Who consumes |
|-------|-----------------|--------------|
| `CoreEvent::Protocol(ServerNotification)` | ~70 wire-tagged lifecycle events (turn, item, sub-agent, MCP, compaction, plan, cost, sandbox, hooks, …) | TUI, SDK, bridge — everyone |
| `CoreEvent::Stream(AgentStreamEvent)` | Hot per-token deltas + tool-call lifecycle (6 variants) | TUI direct; SDK via `StreamAccumulator` |
| `CoreEvent::Tui(TuiOnlyEvent)` | Overlays, toasts, terminal-only | TUI only; dropped by SDK / bridge |

Producers: `QueryEngine` (main loop), `TaskManager`, hooks, retrieval
(isolated stream), sub-agent coordinator.

This is materially richer than `codex-rs`'s `EventMsg` / `RolloutItem`,
because it was designed up-front for "many consumers, three transport
shapes". **The hub should slot in as one more consumer layer, not
require remodelling the event taxonomy.**

### 2.2 What is persisted today

- **Session transcripts**: `~/.coco/projects/<slug>/<session_id>.jsonl`
  via `app/session/storage.rs`. Append-only. Stores message-shaped items
  (`user`, `assistant`, `system`, `attachment`, `tool_result`) + a small
  set of metadata sidecars (custom title, tags, cost, file snapshots).
  This is **messages-only**, not the event firehose.
- **Concurrent-session registry**: one JSON file per PID in
  `~/.coco/sessions/`, used to enumerate live sessions.
- **No SQLite anywhere.** Searched the tree: no `rusqlite` / `sqlx` /
  `sqlite` references in `coco-rs/`.
- **No event-stream persistence.** `Stream` / `Tui` layer events are
  transient; even `Protocol` events are not logged — they hit the
  consumer and are gone.

### 2.3 What `coco-rs` already exposes externally

- **TUI**: in-process, ratatui, single session.
- **SDK / CLI server mode** (`app/cli/src/sdk_server/`): NDJSON over
  stdio (`--json` flag). Drops `Tui` layer; runs `Protocol + Stream`
  through `StreamAccumulator` → `ThreadItem` and serializes for the
  SDK consumer.
- **Bridge** (`bridge/`): REPL bridge, IDE bridge. Currently stdio
  NDJSON (`ControlRequest`/`BridgeOutMessage`); CLAUDE.md mentions
  SSE / WS transports, but the baseline is stdio.
- **No HTTP server**: `axum` is in the tree but only as a `rmcp-client`
  transport dep, not as a `coco` server.

### 2.4 Gap, in one sentence

`coco-rs` produces a **richer event stream than `codex-rs`** but
**persists less** (messages only, not events) and **exposes nothing over
the network** (everything in-process or stdio).

---

## 3. Decomposing the user's scenario

The scenario, restated as discrete pressure points:

| # | Pressure | Implication |
|---|----------|-------------|
| S1 | "Agent runs are getting longer and more complex" | We need a durable record of the full execution, not just messages — tool inputs/outputs, sub-agent spawns, hook decisions, compaction, plan-mode transitions. |
| S2 | "TUI only suits simple UI" | The product surface for analysis must be **outside the agent process**, with a rendering tech (web) that supports rich tables, search, graphs, time-series. |
| S3 | "Need a web UI for interactive analysis + search" | Two distinct features: (a) rendering, (b) query. Query implies an index, not just blob storage. |
| S4 | "Support distributed" | One hub, many agents. Agents may be on different hosts. |
| S5 | "Connector → remote HTTP" | The agent-side egress must speak HTTP; the hub must accept HTTP. WebSockets / SSE are upgrades on top, not the baseline. |
| S6 | "Multiple instances, managed by instance" | The hub needs **instance identity** as a first-class concept, separate from session id. |
| S7 | "Phase 1: events only" | Phase-1 scope = one-way event stream. Don't over-build. |
| S8 | "Architecture preserves long-link control flow" | The wire format must not foreclose adding a bidirectional channel. Implication: do not bind the design to a request/response idiom. |
| S9 | "Single-machine all-in-one via CLI flag" | The server must build into the `coco` binary; you must be able to run hub + agent in one process for the laptop case. |

Reframed cleanly, we are building **three things at once**:

1. **An event egress** inside `coco-rs` (the *connector*).
2. **A receiving / storing / serving daemon** (the *hub*) that can run
   standalone or be linked into `coco` for the all-in-one case.
3. **A web UI** that the hub serves.

Each can evolve on its own cadence, **provided** the contract between
them — a stable wire protocol — is designed up front.

---

## 4. Non-functional requirements (derived)

These weren't asked for explicitly, but follow from the scenario:

- **N1 — Non-blocking egress.** The connector must never wedge the agent
  on a slow / down hub. Strategy choices: drop-oldest, spool-to-disk,
  bounded buffer with backpressure to a side channel — all are options
  for round 2.
- **N2 — Crash safety.** A `coco` crash mid-event must not lose
  everything since last fsync. If we spool to disk, that file is the
  recovery boundary.
- **N3 — Order preservation per session.** The hub must show events in
  the order the agent emitted them, even with retries. `mpsc` already
  guarantees this per-sender; the wire protocol must carry a monotonic
  sequence number so the hub can detect gaps.
- **N4 — Schema evolution.** Today's `ServerNotification` enum will
  change. The hub must accept "unknown variant" and degrade gracefully.
- **N5 — Secret redaction at source.** Reuse `utils/secret-redact`
  before the event leaves the process.
- **N6 — Authn/Authz**, at least optional from day 1. Even local
  all-in-one mode must not be openly listening on `0.0.0.0` by default.
- **N7 — Forward-compat with control flow** (S8). The wire shape must
  allow a *response* channel on the same connection (Phase 3 WS).
- **N8 — Multi-tenant isolation.** One hub serving many users/agents
  must not let one tenant read another's stream.
- **N9 — Observability of the connector itself.** Metrics:
  `events_sent_total`, `events_dropped_total{reason}`,
  `connector_buffer_depth`, `hub_post_latency_p95`. Reuse existing
  `common/otel`.

---

## 5. Proposed integration boundaries (sketch, not a design)

This is the **shape** of the integration, not a commitment to specific
crate names or types. Each numbered choice is open for round 2.

```
┌─────────────────────── coco-rs process ───────────────────────┐
│                                                                │
│  QueryEngine ──emit_*──> mpsc::Sender<CoreEvent> ──┬──> TUI    │
│                                                     ├──> SDK    │
│                                                     ├──> Bridge │
│                                                     └──> [NEW]  │
│                                                         Event-  │
│                                                         Hub     │
│                                                         Sink    │
│                                                          │      │
│                                                  redact + batch │
│                                                          │      │
│                                                  HTTP POST /v1/ │
│                                                  events         │
└──────────────────────────────────────────────────┬─────────────┘
                                                   │
                  ┌────────────────────────────────┼────────────┐
                  │ Event Hub (separate process    ▼            │
                  │   or in-proc via CLI flag)                  │
                  │                                              │
                  │   Ingest API  ─►  Instance registry          │
                  │       │                                      │
                  │       ▼                                      │
                  │   Storage (TBD: SQLite? Parquet?             │
                  │             JSONL+SQLite like codex?)        │
                  │       │                                      │
                  │       ▼                                      │
                  │   Query API  ──►  Web UI (HTML/JS bundle     │
                  │                    served from same daemon)  │
                  │                                              │
                  │   [Phase 3] WS upgrade ◄── Control plane    │
                  └─────────────────────────────────────────────┘
```

Reference crate layout, names **placeholder**:

- `coco-rs/services/event-hub-sink/` — connector library (depends on
  `coco-types`, `utils/secret-redact`, `common/otel`).
- `coco-rs/app/event-hub-server/` (or a new top-level `coco-rs/hub/`)
  — the server. Binary + library. Behind a Cargo feature so the default
  `coco` build doesn't pay for it; `--features hub-embedded` (or
  similar) compiles it into the CLI for all-in-one mode.
- `coco-rs/common/event-hub-protocol/` — pure wire types (`IngestRequest`,
  `IngestAck`, `InstanceDescriptor`, `EventEnvelope`, version
  negotiation). No transport, no storage. Both sides depend on this.

**Why a protocol crate even for V1**: the moment we have one connector +
one server, the wire is a contract. Splitting it into its own crate
keeps the server free to swap its storage and the connector free to
swap its transport.

---

## 6. Open questions for round 2

These are the decisions that meaningfully shape the design. Each one
needs an answer (or an explicit "we'll defer") before we start coding.

| # | Question | Why it matters | Default lean |
|---|----------|---------------|--------------|
| Q1 | **Storage backend** — SQLite (match `codex-rs`), JSONL + SQLite (two-tier), Parquet/DuckDB (OLAP-friendly), embedded RocksDB? | Drives the whole server. SQLite is boring + battle-tested + matches the reference. Analytics workloads (cross-session aggregate) may want OLAP later. | **SQLite for V1.** Two-tier (event JSONL + SQLite index) only if we hit a wall on file size. |
| Q2 | **Event scope** — Protocol only, Protocol+Stream, or everything including Tui? | Stream is high-frequency (per-token deltas). Persisting verbatim could 10× the volume; aggregating may lose info. Tui events are pointless to send. | **Protocol + a sampled / coalesced Stream** (e.g. one `TextDelta` per N tokens or per K ms). Tui never sent. |
| Q3 | **Identity model** — what is an "instance"? Per-host? Per-`coco`-process? Per-session? Hierarchical? | Determines URL structure, partitioning, the web UI's primary nav. | **`(tenant, instance, session, agent)` 4-tuple.** Instance = a `coco` process lifetime; session = a top-level run; agent = sub-agent within. |
| Q4 | **Wire envelope format** — JSON over HTTP, NDJSON streamed POST, protobuf, MessagePack? | Compatibility, debuggability, throughput. | **NDJSON streamed POST** (one event per line, keep-alive connection). Debuggable + supports a single long-lived upload. Upgrade to WebSocket later for bidirectional. |
| Q5 | **Buffering policy** — drop-oldest, spool-to-disk, block? | Trade-off between memory, durability, and latency. | **Bounded in-memory ring + spool-to-disk on overflow.** Drop only as absolute last resort, with a counter. |
| Q6 | **Auth** — none / shared secret / per-instance token / mTLS? | All-in-one mode wants zero config; production wants strong auth. | **Bearer token** (env-driven, optional); refuse non-loopback bind without a token. |
| Q7 | **Schema versioning** — header? per-event? capability negotiation? | We *will* break the schema. Determines forward-compat strategy. | **`X-Coco-Protocol-Version` on the ingest stream + per-event `schema_version`**, hub stores raw + parsed and tolerates unknown variants. |
| Q8 | **Web UI tech** — server-rendered HTML, SPA (React/Svelte), embedded `egui` web? | Affects build complexity, what the hub binary ships. | **SPA built into a single JS bundle**, served from the hub binary via `include_dir!`. No external CDN deps. |
| Q9 | **Retention** — TTL, size cap, manual purge? | A laptop user shouldn't fill their disk; a team server may want long history. | **Configurable, default 30 days + 10 GB cap.** Per-instance overrides. |
| Q10 | **Sub-agent topology** — re-derive from events (`SubagentSpawned`) or require explicit parent pointer? | Determines whether the UI's tree view is computed or stored. | **Re-derive from events at ingest**, store materialized edges in a separate table (mirrors `codex-rs/agent-graph-store`). |
| Q11 | **Phase-3 control plane shape** — WebSocket per session? One WS per instance with multiplexing? gRPC bidi? | Avoid baking in a Phase-1 decision that boxes us in. | **WS upgrade on the same HTTP host**, one WS per instance, frames carry `(session_id, request_id, payload)`. Document only — don't build. |
| Q12 | **Reuse vs build** — is there an off-the-shelf piece (OpenTelemetry collector + Jaeger? Phoenix? LangSmith local?) that does enough? | If yes, we'd add an OTLP exporter instead of building a server. | **Build, but with an OTLP-shaped envelope** so the same events could feed an OTel collector later. Custom UI is the main reason — generic tracing UIs don't model agent events well. |
| Q13 | **CCR / TS reference** — does Claude Code have any comparable backend we should align with? | Avoid drifting from product reality. | **No known equivalent.** Treat as net-new. |

---

## 7. Suggested phase roadmap (subject to round 2)

| Phase | Scope | Definition of done |
|-------|-------|--------------------|
| **0 — Spec freeze** | Round-2/3 docs: pick storage, schema, identity, auth, wire format. | `02..05.md` filed; one open issue per Q1–Q13 closed. |
| **1 — Egress + minimal ingest** | Connector crate; HTTP ingest endpoint; SQLite store; instance registry; minimal "session list + event timeline" web UI; all-in-one CLI flag. | `coco --serve-hub --port 8080` works on a laptop; opening `localhost:8080` shows a live session's event timeline; another `coco` process feeds the same hub over HTTP. |
| **2 — Analytics surface** | Cross-session search; filter by tool / sub-agent / error; cost rollups; tree-view of sub-agent spawn graph; durable replay. | Operator can answer "show me all failed `Shell` tool calls across all my sessions in the last week" from the UI. |
| **3 — Control plane** | WS upgrade on the hub; control-message protocol (cancel turn, deny tool, inject user input, change permission mode); web UI gains an "act" button per session. | A user can drive a remote `coco` from the web UI without ever opening its terminal. |
| **4 — Federation / scale** | Multi-hub, S3-backed cold storage, OTLP export, retention enforcement. | Out of scope until we feel real pressure here. |

---

## 8. What this round explicitly does **not** decide

- Crate names (`event-hub-sink`, `coco-hub`, etc. are placeholders).
- Whether the hub is a separate binary or a `coco` subcommand.
- The exact `EventEnvelope` schema.
- The web UI stack.
- The storage engine.
- The auth model.

All of those are round-2+ topics. The intent of round 1 is to make sure
**we agree on the shape of the problem and the scope** before we argue
about the shape of the solution.

---

## 9. Next-step proposal

Round 2 should be a focused decision pass on **Q1, Q3, Q4, Q7** — these
four answers compose the wire-and-storage contract that everything else
sits on top of. The remaining questions can be resolved later without
breaking what we'd already have built.

Suggested artifact for round 2: `02-architecture-sketch.md` containing
the resolved answers to Q1/Q3/Q4/Q7 plus a strawman crate layout the
team can attack.
