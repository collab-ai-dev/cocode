# Round 2 — Decisions Log

> Three load-bearing decisions made this round. Each is recorded with the
> *rationale*, what it now **pins down**, and the **residual refinement
> questions** it leaves for round 3. The intent is to compress the round-1
> open-question matrix to a tractable shape, not to spec the full design.

## Verified facts from `coco-rs` (round-2 fact-check)

These were confirmed by reading source before finalizing decisions below:

| Fact | Source |
|------|--------|
| `/clear` **rotates `session_id`** — generates a fresh `Uuid::new_v4()` and propagates it to id-keyed subsystems so post-clear writes don't land in the old session's directory | `app/cli/src/session_runtime.rs:2706-2707` (`clear_conversation` → `adopt_session_id`) |
| The session registry `~/.coco/sessions/<pid>.json` is written by `SessionRegistration` with fields: `pid`, `session_id`, `cwd`, `started_at`, `kind`, `entrypoint?`, `name?`, `bridge_session_id?`, `updated_at?`, `status?`, `waiting_for?`. **No `instance_id` field today.** | `app/session/src/concurrent_sessions.rs:94-116` |
| TUI status bar (`render_status_bar`) already renders 8 indicator chips: model · thinking · permission · chord · tokens · ctx% · mcp · msgs | `app/tui/src/surface/viewport.rs` + `app/tui/src/presentation/footer.rs` |

**Implication for D2**: because `session_id` rotates on `/clear`, a single
process lifetime can contain *multiple* `session_id` values in sequence. The
hub must therefore model `instance` and `session` as **two separate levels of
identity**, not one composite. This is what motivates D2's revised shape below.

---

## D1. No agent-side database. Session JSONL stays the only on-disk source of truth.

> *"agent 不需要引入 db，当前 session 已经持久化到目录了"*

### What this means

- The connector inside `coco-rs` is **purely a network egress** — no
  SQLite, no spool file, no event journal.
- The existing `~/.coco/projects/<slug>/<session_id>.jsonl` transcript
  (owned by `app/session/storage.rs`) is **untouched**. It already
  persists user/assistant/system/attachment/tool_result. That's enough
  for the agent to resume itself.
- If the hub is unreachable, events are **lost** — not buffered to
  disk. The agent's correctness does not depend on the hub.

### What this resolves from round 1

| Round-1 Q | Now resolved as |
|-----------|-----------------|
| Q5 — Buffering policy | **Bounded in-memory ring only.** No disk spool. On overflow, drop oldest + bump a counter + emit a marker. |
| (implicit) — backfill on hub recovery | **No backfill.** When the hub reconnects, it starts seeing events again from that point. Don't replay history from JSONL. |
| (implicit) — agent fail-open vs fail-closed | **Always fail-open.** A misconfigured/unreachable hub never blocks a turn, never produces a user-facing error. |

### Residual refinement questions

- **R1.1** Ring-buffer size? Suggest **default 10 000 events / configurable**.
  Sized for ~1 minute of intense streaming at observed rates.
- **R1.2** **Accepted.** Connector emits a synthetic `EventsDropped { count, since_seq, until_seq }` marker over the wire so the web UI can render a "gap here" visual.
- **R1.3** Retry policy on network errors? Suggest **exponential backoff
  with cap (e.g. 100ms → 30s)**, queue continues to fill, drops only
  when full.
- **R1.4** TUI "hub offline" status — detailed analysis now in **Appendix B**. **Recommended: defer to V2** (post-hoc visibility via `EventsDropped` markers is enough; revisit when distributed users report confusion).

### Tradeoffs we are explicitly accepting

- Events are **lost** during hub downtime. The user's scenario
  ("track the whole long run") tolerates this because:
  - Messages are still in the session JSONL.
  - The hub is meant to be highly available — in all-in-one mode it
    is the same process, so "downtime" is just "agent crashed too".
- We are **not** building a durable agent-side event log. If that
  becomes a real need, it can be added later as an opt-in feature
  without changing the wire protocol.

---

## D2. Instance identity = **one opaque `instance_id` bound to process lifecycle**, all other fields are *attributes*.

> *"如果再增加 1 个和启动生命周期绑定的 instanceid 是否足够了？其他的是这个 instance 的属性"* — user, round 2 follow-up.

### What this means

- An "instance" = **one `coco` process lifetime**, identified by a fresh
  `Uuid::new_v4()` minted at process start and held in memory until exit.
- This `instance_id` is **persisted into the existing session registry**:
  add one field to `SessionRegistration` (`app/session/src/concurrent_sessions.rs:94-116`).
- Everything else — `cwd`, `pid`, `started_at`, `version`,
  `peer_protocol`, `kind`, `entrypoint`, `name`, host — is an
  **attribute** of the instance, sent once at announce time, queryable
  in the hub but not part of the identity.

### Why this is the right shape (refining round-2's first sketch)

The earlier sketch — identity = composite of `(host, cwd, start_time, pid)`
— was over-engineered. The user's revision is cleaner:

- **Identity ≠ description.** A UUID is opaque; attributes describe.
  Mixing them into one composite hash means every attribute change
  becomes an identity change (or you have to be careful about which
  fields go into the hash). Separating them is what every mature
  registry-like system does (k8s UIDs, AWS resource IDs, etc.).
- **Zero collision risk.** A v4 UUID at process start has no failure
  modes the composite hash would have rescued.
- **No hashing on either side.** Hub stores the UUID directly; agent
  declares it.
- **The session registry already exists.** We piggyback on infrastructure
  the user already built — one extra field, no new file.

### Hierarchy

Because `/clear` rotates `session_id` (verified, see top), one instance can
contain multiple sessions in sequence:

```
instance_id = a7f3-…              ← born at `coco` startup, dies on exit
  ├─ session_id = 9c19-…          ← initial session
  │   └─ (sub-agents, by AgentId)
  ├─ session_id = 4ee2-…          ← after first /clear
  └─ session_id = 7b18-…          ← after second /clear
```

Hub URL space mirrors this:
- `/i/<instance_id>` — instance overview (all sessions for this process)
- `/i/<instance_id>/s/<session_id>` — one session's timeline

### Concrete schema delta

**Wire** (sent once per instance, on connector startup):

```rust
pub struct InstanceAnnounce {
    pub instance_id: Uuid,
    pub attributes: InstanceAttributes,
}

pub struct InstanceAttributes {
    pub host: String,             // hostname() at startup
    pub cwd: PathBuf,             // coco_paths.cwd at startup
    pub pid: u32,
    pub started_at: i64,          // unix ms (already in SessionRegistration)
    pub version: String,          // coco-rs version
    pub kind: SessionKind,        // existing enum
    pub entrypoint: Option<String>,
    pub name: Option<String>,
}
```

**Local** (the existing `SessionRegistration` gains one field):

```rust
pub struct SessionRegistration {
    pub instance_id: Uuid,        // ← NEW (constant for process lifetime)
    pub pid: u32,
    pub session_id: String,       // ← still rotates on /clear
    pub cwd: PathBuf,
    pub started_at: i64,
    // … existing fields unchanged
}
```

**Every event envelope** carries both IDs:

```rust
pub struct EventEnvelope {
    pub instance_id: Uuid,        // stable for process lifetime
    pub session_id: String,       // rotates on /clear within an instance
    pub seq: u64,                 // monotonic per (instance, session)
    pub ts: DateTime<Utc>,
    pub payload: …,
}
```

### Round-3 follow-ups

- **R2.4** Naming: `instance_id` (snake_case in JSON to match existing
  registry `session_id`, `started_at`)?
- **R2.5** When `coco` is launched in **daemon worker** mode (a worker
  process spawned by an interactive parent), is the worker a separate
  instance or part of the parent's? Recommend **separate instance**
  because the worker has its own process lifetime, but link via a
  `parent_instance_id` attribute. (Defer to round 5 if too speculative.)

---

## D3. The hub is "TUI in a browser" + search index. Not a control plane in V1.

> *"http 主要是 tui 1个更丰富的ui表现形式，而且在 httpserver 侧进行数据存储，用于搜索"*

### What this means

- The hub is **a projection of the same data the TUI consumes**,
  rendered in HTML/CSS instead of `ratatui`, with an index on top so
  you can search.
- **Storage on the hub is for search**, not for replay. The agent
  remains the authority on its own session.
- **Phase 1 is observation-only.** No "act from the web UI" — no
  cancel, no approve, no inject. (Round-1 Q11 / "Phase-3 control
  plane" is deferred to a later document.)

### What this resolves from round 1

| Round-1 Q | Now resolved as |
|-----------|-----------------|
| Q11 — Phase-3 control plane shape | **Out of V1 scope.** Wire protocol must leave room (D3.x below), but the design is parked until phases 1+2 ship. |
| Q12 — Reuse vs build (OTLP, Jaeger, …) | **Build.** Generic tracing UIs don't model agent events (sub-agent topology, plan-mode transitions, hook decisions) and we have a richer source than what OTLP carries. We can still add OTLP *export* later. |
| (implicit) — hub-side storage purpose | **Search + browse, not session truth.** Storage is a *projection*; the agent's session JSONL remains canonical for that one session. |

### Forward-compat clauses we hold onto

Even though Phase 1 is read-only, three things in the wire protocol
must not foreclose Phase 3:

- **D3.a Wire transport is HTTP/1.1 with NDJSON streaming POST**, *not*
  one-shot request/response. A streaming POST is a natural precursor
  to a WS upgrade on the same endpoint — Phase 3 can flip a single
  endpoint from "stream up only" to "duplex" without protocol churn.
- **D3.b Every event carries a monotonic `seq` per `(instance,session)`**.
  This lets future control messages reference "in response to seq N"
  without needing a separate causality scheme.
- **D3.c The hub already addresses sessions individually** in its
  URL space (`/i/<instance>/s/<session>`), so when the time comes,
  WS connections per-session "just work".

These are minimal commitments — they cost nothing in V1 and unlock
Phase 3 cheaply.

### Residual refinement questions

Detailed analysis of the projection rule (R3.1) and search shape (R3.2) is
now in **Appendix A** at the end of this document.

- **R3.1** Projection rule — see Appendix A. **Recommended: "Option A
  (per-turn aggregation)" for V1, with the door open for Option B
  (periodic flush) if live-watch UX becomes a real ask.**
- **R3.2** Search shape — facets + FTS5; deferred to round 4.
- **R3.3** Web UI tech — deferred to round 5; tentatively SPA bundled
  via `include_dir!`.

### Storage scope, restated for clarity

The hub stores:
- Everything in the projection (R3.1), denormalized for search.
- Materialized sub-agent edges (re-derived from `SubagentSpawned`
  events at ingest time — matches `codex-rs/agent-graph-store` shape).
- Instance and session metadata (open/closed, last activity, model,
  cost rollup).

The hub does **not** store:
- The session JSONL (the agent owns that locally).
- Anything that would let the hub *resume* the agent (it can't, by D1).

---

## Architectural picture after these decisions

```
┌──────────────────────── coco-rs process ────────────────────────┐
│                                                                  │
│  [UNCHANGED]  session JSONL writer                              │
│    └─ ~/.coco/projects/<slug>/<session_id>.jsonl   (messages)   │
│                                                                  │
│  [NEW]  event-hub connector                                     │
│    ├─ subscribes to mpsc::Sender<CoreEvent> (new sink slot)     │
│    ├─ filters: Protocol fully + Stream aggregated, Tui dropped  │
│    ├─ redacts secrets (utils/secret-redact)                     │
│    ├─ in-memory ring buffer (bounded, drop-oldest on overflow)  │
│    ├─ HTTP NDJSON streaming POST to /v1/events                  │
│    └─ identity: (host, cwd, start_time, pid) → instance_id      │
└───────────────────────────────┬──────────────────────────────────┘
                                │ HTTP/1.1, streamed NDJSON
                                ▼
┌──────────────── event-hub server (standalone or embedded) ──────┐
│                                                                  │
│  Ingest:  POST /v1/events    (NDJSON, long-lived per session)   │
│           POST /v1/announce  (instance descriptor on startup)   │
│                                                                  │
│  Storage: SQLite                                                │
│           ├─ instances(host, cwd, start_time, pid, …)           │
│           ├─ sessions(instance_id, session_id, model, …)         │
│           ├─ events(instance_id, session_id, seq, kind, ts, …) │
│           ├─ agent_edges(parent_agent_id, child_agent_id, …)   │
│           └─ FTS5 virtual table over event content              │
│                                                                  │
│  Query :  GET  /v1/instances                                    │
│           GET  /v1/sessions?instance=…                          │
│           GET  /v1/events?session=…&kind=…&q=…                  │
│                                                                  │
│  Web UI: SPA at /, fetches from /v1/*                           │
│                                                                  │
│  Run modes:                                                      │
│    standalone:  `coco-hub --port 8080`                          │
│    embedded:    `coco --serve-hub --port 8080`                  │
└──────────────────────────────────────────────────────────────────┘
```

---

## Compressed open-question set for round 3

The round-1 matrix shrinks substantially. What's left:

| Round-1 Q | Status after round 2 |
|-----------|----------------------|
| Q1 — Storage backend (hub side) | **Tentatively SQLite + FTS5** (D3, R3.2). Confirm in round 4. |
| Q2 — Event scope | **Resolved by R3.1** (Protocol+aggregated Stream). Round-3 confirms the aggregation rule. |
| Q3 — Identity model | **Resolved by D2** (host, cwd, start_time, pid). |
| Q4 — Wire envelope | **Tentatively NDJSON streaming POST** (D3.a). Confirm the per-event schema in round 3. |
| Q5 — Buffering | **Resolved by D1** (in-memory ring, drop oldest, marker event). |
| Q6 — Auth | **Open.** Recommend bearer token (loopback may go tokenless when bound to 127.0.0.1). Decide in round 5. |
| Q7 — Schema versioning | **Round 3** — pair with wire envelope decision. |
| Q8 — Web UI tech | **Round 5.** SPA into binary by default. |
| Q9 — Retention | **Round 4** — alongside storage. |
| Q10 — Sub-agent topology | **Resolved** — re-derived at ingest, materialized in `agent_edges`. |
| Q11 — Control plane | **Deferred** by D3. |
| Q12 — Reuse vs build | **Resolved by D3** — build, with optional OTLP export later. |
| Q13 — TS / CCR reference | **Resolved** — no equivalent. |

---

## Round-3 suggested focus

Write **`03-wire-protocol-and-schema.md`** covering:

1. The exact NDJSON envelope: `EventEnvelope { instance, session, seq, ts, schema_version, payload }`.
2. The aggregation rule for `Stream` events (R3.1).
3. The `instance announce` request body (full `InstanceDescriptor`).
4. Schema-version negotiation header (`X-Coco-EventHub-Protocol: 1`).
5. The `events_dropped` marker payload (R1.2).
6. Error semantics on the ingest path (what counts as retriable; the
   server must accept "unknown variant" gracefully).

That document is the **contract**. Once it's stable, the connector
crate, the protocol crate, and the hub crate can all proceed in
parallel.

---

## Items to confirm before round 3

All four resolved:

- ✅ **R2.1 → superseded by D2 revision** — identity is a single opaque
  `instance_id` UUID; `host`/`cwd`/`pid` become *attributes*, not
  identity components.
- ✅ **R1.2** — `EventsDropped` marker accepted.
- ✅ **R3.1** — **Option A** confirmed: per-turn aggregation. No
  streaming/chunked text events on the wire in V1. (User: *"不需要支持
  stream，按轮发送就行"*.) Appendix A keeps the rationale.
- ✅ **R1.4** — **Deferred** out of V1. TUI does not gain a hub-status
  chip. Post-hoc visibility via `EventsDropped` markers is the only
  signal. (User: *"当前 tui 不需要显示 hub 的状态"*.) Appendix B kept for
  record.

Round 3 can now proceed against a fully closed decision set.

---

## Appendix A — R3.1 in detail: what does "aggregate Stream" actually mean?

### A.1 Why this question is hard

`CoreEvent` has three layers, but the *volume* and *value-per-event* differ
by 100× across them. A naive "send all events" floods the wire; a naive
"drop streaming" loses the substance of what the model produced. The
projection rule has to reconcile those.

| Layer / variant | Approx volume per turn | Value per event in isolation | Value when aggregated |
|---|---|---|---|
| `Protocol(_)` — 70+ lifecycle variants | 10–50 | **high** (each is a state change) | n/a |
| `Stream(TextDelta)` | 200–5000 (1/token) | very low | **high** (the full text) |
| `Stream(ThinkingDelta)` | 100–2000 (1/token) | very low | **high** (the full reasoning) |
| `Stream(ToolUseQueued/Started/Completed)` | 3–30 (3 per tool call) | **high** | n/a |
| `Stream(McpToolCallBegin/End)` | 0–20 | **high** | n/a |
| `Tui(_)` | 10–100 | zero outside terminal | zero |

Two layers are easy:
- **Protocol → send all.** Low volume, every event is a meaningful state
  change. This is exactly what the search/analysis surface needs.
- **Tui → never send.** It exists only because TUI consumes the same
  channel as SDK does, and overlays/toasts/spinners would be noise in a
  web UI.

The tricky layer is `Stream`. Its tool-call variants are like Protocol
(low volume, high value, send all). Its `TextDelta`/`ThinkingDelta` are
the problem: thousands of near-empty events whose individual content is
worthless but whose concatenation is the actual model output.

### A.2 Three concrete aggregation strategies for TextDelta/ThinkingDelta

**Option A — Per-turn aggregation (recommended for V1)**

The connector keeps a per-turn buffer. Each `TextDelta` and
`ThinkingDelta` is appended to a string. On `Protocol(TurnCompleted)`
(or `TurnFailed`/`TurnInterrupted`), the connector emits two synthetic
events to the wire:

```
TextBlockCompleted   { turn_id, full_text,    block_index, char_count }
ThinkingBlockCompleted { turn_id, full_text,  block_index, char_count }
```

`block_index` lets us distinguish multiple text blocks in one turn (e.g.
text → tool → text), preserving the order the model produced them.

| | Pros | Cons |
|---|---|---|
| **Option A** | Lowest event count. Hub stores final text once, indexes once, displays once. Simplest. | Web UI shows "(generating…)" placeholder until the turn completes. For a 60-second generation that's a long blank stare. |

**Option B — Periodic-flush aggregation**

The connector flushes the accumulated buffer every N ms (default
500–1000 ms) as `TextChunk { turn_id, block_index, delta_since_last,
seq_within_block }`. At turn end, emits a final
`TextBlockCompleted { turn_id, block_index, full_text }` so the hub has
a canonical, indexable string.

| | Pros | Cons |
|---|---|---|
| **Option B** | Web UI gets progressive updates — text appears as the model thinks. Resilient to mid-turn crashes (partial text survives). | More events. Hub stores partials + final → either deduplicate at ingest, or accept slight storage redundancy. |

**Option C — Size+time hybrid**

Flush when accumulated text reaches K bytes OR T ms elapses, whichever
first. Bounds both latency and event count.

| | Pros | Cons |
|---|---|---|
| **Option C** | Bounded under any workload. | Two knobs to tune; little material difference from Option B in practice. |

### A.3 Which one for V1?

**Recommendation: Option A.** Reasons:

1. The user framed the hub as "richer UI for TUI + search". Search wants
   completed text, not streaming chunks. Completed text is the natural
   primary store.
2. The TUI is still the live-driving surface. Operators watching a turn
   stream live will use the TUI; web-UI users mostly look retrospectively.
3. Simplicity. The hub stores one row per completed text block; no
   delta-merge logic.
4. Cost of being wrong is low: if users complain about staleness during
   long generations, we **add** the chunked path in V2 without breaking
   the existing one (Option A's `TextBlockCompleted` keeps working;
   Option B's `TextChunk` is purely additive).

The single risk of Option A is mid-turn crash: if the agent dies before
`TurnCompleted`, the wire never sees the text. But:
- The hub has already seen `Protocol(TurnStarted)`, so it knows a turn
  began.
- The session JSONL on the agent's disk has whatever the agent committed.
- Operators can reconstruct from there if it matters.

### A.4 Cross-cutting rules (apply regardless of A/B/C)

- **Tool input/output**: send full payload. These are the substance of
  what tools did. The connector redacts secrets before serializing.
  Truncation policy (e.g. "tool output > 1 MB stored as blob ref")
  belongs in round 4 alongside storage.
- **Sub-agent text**: same projection rule, applied per sub-agent.
  Sub-agent text is attributed via the existing `AgentId` from `coco-types`.
- **Reasoning vs answer text**: kept as separate blocks
  (`ThinkingBlockCompleted` vs `TextBlockCompleted`) so the UI can fold
  reasoning away by default.
- **Streaming ordering inside a turn**: the per-`(instance, session)`
  monotonic `seq` (D3.b) is what the hub uses to order. The synthetic
  aggregated events take the `seq` of the *last* delta they consumed,
  preserving causality with following Protocol events.

### A.5 Volume estimate after projection

For a typical 30-turn coding session with 5 tool calls per turn and
average 500 tokens of assistant output per turn:

| Stream variant | Raw events | After Option A projection |
|---|---|---|
| TextDelta | ~15 000 | ~30 (one per turn) |
| ThinkingDelta | ~3 000 (if thinking enabled) | ~30 |
| ToolUseQueued/Started/Completed | ~450 | 450 (unchanged) |
| Protocol events | ~600 | 600 (unchanged) |
| Tui events | ~600 | 0 |
| **Total to hub** | **~19 650** | **~1 110** |

~18× reduction with no loss of analytical signal. Storage and the search
index both benefit by the same factor.

---

## Appendix B — R1.4 in detail: "hub: offline" chip in the TUI?

### B.1 What this would actually be

A 9th chip on the existing TUI status bar (`surface::viewport` plus
`presentation::footer`, which today shows model, thinking, permission, chord,
tokens, ctx%, mcp, msgs). Probably one of:

- **Dot variant**: a small colored dot — `●` green / yellow / red — next
  to a label like `hub`.
- **Word variant**: `hub: live` / `hub: degraded` / `hub: offline`.

States:
- **live** — connector's last POST succeeded, no events dropped.
- **degraded** — buffer >50% full or some `EventsDropped` markers
  generated.
- **offline** — last N attempts failed or transport returned non-2xx.
- **disabled** — `hub_url` not configured. **Render nothing.** Default
  for users who never enable the hub.

### B.2 Cost to build

| Area | Estimate |
|------|----------|
| Connector status enum + state machine | ~30 LOC |
| Wire status into `AppState` (existing mpsc pattern) | ~40 LOC |
| TUI chip rendering (mirror existing `mcp` chip pattern) | ~30 LOC |
| Tests + insta snapshots | ~50 LOC |
| **Total** | **~150 LOC, 1 day** |

Not free, but not heavy either.

### B.3 Value by deployment scenario

| Scenario | Without chip, how does the operator notice hub is down? | Chip value |
|----------|---------------------------------------------------------|------------|
| **All-in-one** (`coco --serve-hub`) | The hub crashed = the agent crashed (same process). They notice instantly. | **Very low.** |
| **Local hub, local agent** (separate process on same laptop) | Open the web UI → notice no recent events. | **Low.** A 5-second discovery instead of instant; not painful. |
| **Local agent, remote hub** (laptop coco → team server) | Open the web UI when you finally check → notice gap. Could be hours later. | **High.** This is the case where the chip earns its keep. |
| **CI / scripted runs** | Nobody is looking at the TUI. | **Zero.** |
| **SDK-only mode** (`--json`, no TUI) | Logs surface connector retry/drop messages via `tracing`. | **N/A** (no TUI). |

### B.4 The post-hoc alternative (already approved as R1.2)

The `EventsDropped` marker is emitted into the event stream itself.
When the hub does eventually receive events again, it sees the marker
and the web UI renders a visible "gap of N events" indicator. So:

- **For forensics**: covered by R1.2 without the chip.
- **For real-time awareness**: only the chip covers this.

### B.5 Recommendation: defer to V2

Reasons:

1. **Phase 1 should ship.** This is polish, not load-bearing.
2. **R1.2 already gives post-hoc visibility.** The user can always find
   out *that* events were lost, just not always *while* they're being
   lost.
3. **The "remote hub" case isn't day-1 traffic.** The user's stated
   primary scenario is single-machine all-in-one, where the chip's
   value is "very low". Distributed deployments come once the all-in-
   one works.
4. **The TUI is intentionally a thin surface.** Adding chips creates
   pressure to keep adding chips; the design ethos in `coco-rs` so far
   has been to push richness *off* the TUI, which is also the user's
   stated motivation for building the hub.

If after V1 ships there are real reports of "I didn't know my hub was
down for two hours", V2 adds the chip — it's 150 LOC and doesn't break
anything.

**Counter-argument worth recording**: silent failure is a UX smell. If
the user prioritizes "operator should never be surprised by lost data"
over "ship V1 fast", the chip belongs in V1. This is a values call, not
a technical one.
