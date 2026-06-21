# Round 3 — Wire Protocol & Schema

> Status: **draft contract**. Once accepted, this document is the source of
> truth that the connector crate, the protocol crate, and the hub crate
> all build against.
> Depends on: `01-requirements-analysis.md` + `02-decisions-round-2.md`.

The goal of this document is to specify everything you need to be able to
write the protocol crate today without further discussion: every endpoint,
every wire shape, every error code, the aggregation algorithm, the version
negotiation. Storage layout, search index, and web-UI tech are explicitly
out of scope and live in round 4 / round 5.

---

## 1. Decisions inherited

For traceability, repeating the decisions this contract bakes in:

| From | Decision | How it shows up here |
|------|----------|----------------------|
| D1 | No agent-side DB; agent fail-open on hub unreachability | No spool format; only an in-memory ring; `EventsDropped` covers loss visibility |
| D2 | Identity = opaque `instance_id` UUID; everything else is attributes | `announce` frame separates `instance_id` from `attributes`; every event carries `instance_id` + `session_id` + `seq` |
| D3 | Hub = "TUI in browser" + search; no control plane in V1 | One-way event traffic in V1 frames; WS transport already bidirectional so Phase 3 is purely additive |
| R3.1 | Per-turn aggregation, no streaming text on the wire | `TextBlockCompleted` / `ThinkingBlockCompleted` only; no `TextChunk` / `TextDelta` in the public schema |
| R1.2 | Connector emits an `EventsDropped` marker on overflow | Defined as a first-class payload variant |
| R1.4 | TUI gains no hub-status chip in V1 | Not in scope here |
| Round 3 | **Transport = WebSocket (`ws://` / `wss://`)** from V1 | No HTTP/NDJSON intermediate stage; Phase 3 control frames extend the existing WS, not a new endpoint |
| Round 3 | Batch + buffer limits **configurable** | `event_hub_batch_max_events / max_bytes / max_interval_ms` and `event_hub_ring_buffer_size` exposed in settings, env, CLI |
| Round 3 | Connector config keyed on `event_hub_url` | Presence of URL = enabled; no separate `enabled` flag |

---

## 2. Transport

### 2.1 WebSocket (`ws` / `wss`) is the V1 transport

The connector opens **one long-lived WebSocket connection per `coco`
process** to the hub. All event traffic flows over that single
connection. Read-side query endpoints (consumed by the Web UI and
external tools) remain plain HTTP/1.1 + JSON.

Decision rationale (round 3, user-driven):

- **Bidirectional from day 1.** Phase-3 control frames (cancel turn,
  inject user input, approve tool, change permission mode) are just
  additional `kind`s on the same connection — no endpoint creation,
  no protocol upgrade, no client churn.
- **No per-batch HTTP overhead.** A WS text frame carries a single
  batch of envelopes with negligible framing cost; no header
  re-sending.
- **Cleaner reconnect / backpressure semantics.** TCP-level flow
  control handles slow consumers naturally; reconnect is one well-
  defined operation, not a per-batch retry choreography.
- **TLS is `wss://`.** Same TLS stack and cert story as HTTPS.

Trade-offs explicitly accepted:

- Some corporate egress filters drop WebSockets. Mitigation: the all-
  in-one deployment (`--serve-hub`) binds to `127.0.0.1`, immune to
  egress filtering; distributed users hitting WS-hostile networks can
  tunnel.
- Browser network panels show WS frames less ergonomically than HTTP
  request/response pairs. Mitigation: the Web UI's traffic to the hub
  is plain HTTP — only the connector uses WS.

### 2.2 Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| **WS** | `ws[s]://<hub>/v1/connect` | Single connection per connector; all event traffic |
| `GET` | `/v1/instances` | List known instances |
| `GET` | `/v1/instances/{instance_id}` | One instance + attributes + session summary |
| `GET` | `/v1/instances/{instance_id}/sessions` | List sessions |
| `GET` | `/v1/instances/{instance_id}/sessions/{session_id}/events` | List events (paged) |
| `GET` | `/v1/search` | Cross-cutting search (round 4 defines parameters) |
| `GET` | `/healthz` | Liveness probe; `200` if hub can serve |
| `GET` | `/v1/protocol` | Returns supported WS subprotocols and protocol metadata |

**Phase 3 reserves no new endpoints.** Control frames are added as new
`kind`s on the same `/v1/connect` connection. This is the major payoff
of choosing WS now: V1 → V3 is purely additive at the wire level.

### 2.3 Subprotocol negotiation

The WS opening handshake carries the version via the standard
`Sec-WebSocket-Protocol` header:

```
Client → Server:  Sec-WebSocket-Protocol: coco-event-hub.v1
Server → Client:  Sec-WebSocket-Protocol: coco-event-hub.v1
```

- If the server cannot honor any subprotocol the client offers, the
  HTTP upgrade handshake fails with `400 Bad Request`. The connector
  logs the version mismatch and **does not retry on the same URL** until
  reconfigured (avoids hot loops against an incompatible hub).
- Multi-version client: connectors that speak more than one version
  list them oldest-last:
  `Sec-WebSocket-Protocol: coco-event-hub.v2, coco-event-hub.v1`.
- Each payload variant still carries its own `schema_version` field
  (§4.3) so additive variant changes don't bump the WS subprotocol.

---

## 3. Identity & Announce

### 3.1 The announce frame

The connector's **first WS frame after the upgrade handshake MUST be**
an announce. The hub will not accept any other frame kind until it has
seen the announce.

```json
{
  "kind": "announce",
  "instance_id": "a7f3c001-9c19-4dfa-8a46-1d4bded65687",
  "attributes": {
    "host": "macbook.local",
    "cwd": "/Users/linyuzhi/codespace/myagent/codex",
    "pid": 40382,
    "started_at": 1778858719347,
    "version": "2.1.142",
    "peer_protocol": 1,
    "kind": "interactive",
    "entrypoint": "cli",
    "name": "coco-tui-rendering-hardening"
  }
}
```

| Field | Type | Source | Required |
|-------|------|--------|----------|
| `instance_id` | UUID v4 | `Uuid::new_v4()` at `coco` startup | yes |
| `attributes.host` | string | `hostname()` | yes |
| `attributes.cwd` | string (path) | `coco_paths.cwd` at startup | yes |
| `attributes.pid` | u32 | `std::process::id()` | yes |
| `attributes.started_at` | i64 (unix ms) | already in `SessionRegistration.started_at` | yes |
| `attributes.version` | string | `env!("CARGO_PKG_VERSION")` of `coco` | yes |
| `attributes.peer_protocol` | u32 | already in registry | optional |
| `attributes.kind` | enum (`interactive`/`bg`/`daemon`/`daemon_worker`) | already in registry | yes |
| `attributes.entrypoint` | string | already in registry | optional |
| `attributes.name` | string | already in registry | optional |

### 3.2 The `announce_ack` frame (hub → client)

The hub responds with one frame:

```json
{
  "kind": "announce_ack",
  "instance_id": "a7f3c001-9c19-4dfa-8a46-1d4bded65687",
  "hub_version": "0.1.0",
  "first_seen": false,
  "resume_from": null
}
```

| Field | Meaning |
|-------|---------|
| `instance_id` | Echo of the announced ID (sanity check) |
| `hub_version` | Hub build version (informational) |
| `first_seen` | `true` if the hub had never seen this `instance_id` before; `false` on reconnect of a known instance |
| `resume_from` | Reserved (round 5+) — hub may instruct the connector to resend from a specific `seq` after a reconnect |

If the hub **rejects** the announce, it sends an `error` frame (§9.2)
and immediately closes the WS with the matching close code (§9.1).
A rejection is non-retriable on the same configuration — the
connector logs and waits for reconfiguration.

### 3.3 Sessions

Sessions are **not** announced. The hub learns a session exists the first
time it sees an event with a new `session_id`. The
`Protocol(SessionStarted)` event is the natural carrier (it already
exists in `coco-types`); the hub creates a row in its `sessions` table
when it first observes that event. When `SessionEnded` arrives the row
is updated.

If for any reason an event with an unknown `session_id` arrives without
a preceding `SessionStarted` (e.g. connector started mid-session, or
events arrived out of order during retry), the hub creates a session
row with `discovered_via: "out_of_band"` and continues normally. The
agent's session JSONL is the only source of truth for session contents;
the hub just records what it observes.

### 3.4 Persisting `instance_id` locally

The connector adds one field to `SessionRegistration`
(`app/session/src/concurrent_sessions.rs:94-116`):

```rust
pub struct SessionRegistration {
    pub instance_id: Uuid,        // NEW — constant for process lifetime
    pub pid: u32,
    pub session_id: String,       // continues to rotate on /clear
    pub cwd: PathBuf,
    pub started_at: i64,
    // … existing fields unchanged
}
```

`instance_id` is generated once at `coco` startup and written into the
registry file alongside the existing fields. It is **not** updated on
`/clear` (whereas `session_id` is). On process exit the registry file is
deleted, as today.

---

## 4. Event envelope

### 4.1 Outer schema

Every line of NDJSON on `POST /v1/events` is one of these:

```json
{
  "instance_id": "a7f3c001-9c19-4dfa-8a46-1d4bded65687",
  "session_id": "9c19edf9-3dcd-46a3-8a46-1d4bded65687",
  "seq": 12345,
  "ts": "2026-05-16T12:34:56.789Z",
  "schema_version": 1,
  "payload": { … see §5 … }
}
```

| Field | Type | Semantics |
|-------|------|-----------|
| `instance_id` | UUID v4 | Constant for the process lifetime |
| `session_id` | string | Current `session_id`, may rotate within an `instance_id` lifetime |
| `seq` | u64 | **Monotonically increasing per `(instance_id, session_id)`** starting at 0 |
| `ts` | RFC 3339 UTC, ms precision | Connector clock; **wall-clock for display only**, not relied on for ordering |
| `schema_version` | u32 | Version of the inner `payload` variant; bumped when that variant changes |
| `payload` | object | Tagged union, see §5 |

### 4.2 `seq` semantics

`seq` is the only field the hub uses for ordering inside a session. It
is:

- Monotonically increasing within a single `(instance_id, session_id)`.
- Starts at 0 for each new session (i.e. resets on `/clear`).
- Assigned by the connector at the point it enqueues an event into its
  ring buffer — **not** at HTTP-send time, so that batch ordering and
  retries don't affect `seq`.

The hub uses `(instance_id, session_id, seq)` as the primary key of the
events table. Duplicate inserts (from a retried batch) are silent
no-ops. Gaps in `seq` (from drops or out-of-order arrival) are tolerated
— the hub stores what it gets.

### 4.3 `schema_version` semantics

- Defaults to `1`.
- Bumped per **payload variant**, not per envelope, when that variant's
  shape changes. The outer envelope is stable.
- A hub that doesn't recognize a `schema_version` for a known `kind`
  should attempt best-effort deserialization (unknown fields ignored
  via `#[serde(deny_unknown_fields)] = false`) and tag the row
  `parse_status: "partial"` if any field failed.

### 4.4 WS frame framing & batch limits

Every WS **text frame** on `/v1/connect` carries **one JSON object**.
The top-level `kind` field discriminates:

| `kind` | Direction | Meaning |
|--------|-----------|---------|
| `announce` | client → hub | First frame; §3.1 |
| `announce_ack` | hub → client | Response to announce; §3.2 |
| `batch` | client → hub | A batch of envelopes |
| `batch_ack` | hub → client | Optional ack; §4.5 |
| `error` | hub → client | Pre-close diagnostic; §9.2 |
| *(future)* `control_*` | hub → client | Phase-3 control plane |

A `batch` frame:

```json
{
  "kind": "batch",
  "events": [
    { …EventEnvelope… },
    { …EventEnvelope… }
  ]
}
```

Configurable limits (defaults in parentheses; see §8):

- **Max events per `batch` frame** — `event_hub_batch_max_events`
  (default **1000**). Hub close code `4013` if exceeded.
- **Max serialized bytes per WS frame** — `event_hub_batch_max_bytes`
  (default **1 048 576 B = 1 MiB**). Hub close code `4013` if exceeded.
- **Max time pending events may sit before flushing** —
  `event_hub_batch_max_interval_ms` (default **500 ms**). Connector-side
  only; the hub does not police this.

Edge cases:

- If a **single envelope** exceeds `event_hub_batch_max_bytes`, the
  connector sends it as a single-event `batch`. The hub treats the
  byte limit as a *batching target*, not a hard frame cap — it accepts
  the oversize frame up to the WS implementation's actual frame
  ceiling. Operators who raise the limit accept the corresponding
  memory cost.
- Binary WS frames are **reserved** (future: compressed batch
  payloads). V1 connectors send only text frames.
- A frame with a `kind` the hub doesn't recognize is logged + ignored
  (not a close-worthy error); a frame the client doesn't recognize
  is logged + ignored.

### 4.5 The `batch_ack` frame (optional, hub → client)

The hub *may* emit `batch_ack` periodically:

```json
{
  "kind": "batch_ack",
  "instance_id": "…",
  "session_id": "…",
  "up_to_seq": 5023,
  "persisted_at": "2026-05-16T12:34:57.000Z"
}
```

Semantics:

- "All events with `seq ≤ up_to_seq` for this `(instance, session)` are
  durably persisted in the hub."
- V1 connectors **do not block** on `batch_ack`. They are purely an
  observability/telemetry signal (hub-reach latency, drop reconciliation).
- Phase-3 features (replay, exactly-once control responses) will rely
  on them — bake them in now so V3 has no protocol churn.
- Hubs that omit acks entirely are conforming (V1 connectors must
  function without ever seeing one).

Hubs deliberately do not return per-event ack lists. If even one
envelope in a batch deserializes, the whole batch is accepted; per-
envelope failures store a row with `parse_status: "failed"` carrying
the raw line for forensics.

---

## 5. Payload variants

`payload` is a tagged union (`#[serde(tag = "kind", rename_all =
"snake_case")]`). All variants in V1:

### 5.1 `protocol`

Pass-through of `coco_types::ServerNotification`. The hub does not
re-encode it; the wire shape is whatever `ServerNotification` serializes
to today, with its existing internal `type` tag.

```json
{
  "kind": "protocol",
  "notification": {
    "type": "turn_started",
    "turn_id": "…",
    "session_id": "…",
    "model": "…"
    // … existing ServerNotification fields …
  }
}
```

Note: this means **any addition to `ServerNotification` is a protocol
change** that must be reflected by either (a) bumping the affected
variant's `schema_version` on the wire, or (b) making the new field
`#[serde(default)]` so older parsers ignore it. Convention: prefer (b)
for additive changes; reserve (a) for breaking renames/removals.

### 5.2 `tool_use_queued` / `tool_use_started` / `tool_use_completed`

Pass-through of the `Stream(ToolUse*)` variants. Tool input/output
payloads are sent verbatim, after `utils/secret-redact` has run.

```json
{
  "kind": "tool_use_completed",
  "call_id": "call_abc123",
  "tool_name": "Read",
  "agent_id": "main",          // or sub-agent AgentId
  "output_text": "…possibly large…",
  "output_meta": { … structured tool result fields … },
  "is_error": false,
  "duration_ms": 42
}
```

Truncation of huge tool outputs (e.g. > 1 MiB) is deferred to round 4
storage policy. V1 connector sends the full output; if that exceeds the
1 MiB batch limit, the batch is split such that one event sits alone in
its own POST.

### 5.3 `mcp_tool_call_begin` / `mcp_tool_call_end`

Pass-through of `Stream(McpToolCallBegin/End)`. Same shape as the tool
variants above but tagged for MCP.

### 5.4 `text_block_completed` / `thinking_block_completed`

The aggregated outputs (R3.1 / D3 / user confirmation: per-turn,
no streaming).

```json
{
  "kind": "text_block_completed",
  "turn_id": "turn_abc",
  "agent_id": "main",
  "block_index": 0,           // sequential within the turn (0, 1, 2 …)
  "full_text": "…the complete model text for this block…",
  "char_count": 1742,
  "started_at": "2026-05-16T12:34:50.100Z",
  "completed_at": "2026-05-16T12:34:56.789Z"
}
```

`thinking_block_completed` is the same shape with `full_text` carrying
the extended-thinking trace.

`block_index` preserves the order of multiple content blocks within
one turn (text → tool → text → tool → text gives blocks 0, 1, 2 — only
text blocks count; tool calls are their own events). See §6 for the
state machine.

### 5.5 `events_dropped`

Connector-synthesized marker emitted whenever the in-memory ring buffer
overflowed or any other internal drop occurred.

```json
{
  "kind": "events_dropped",
  "count": 137,
  "since_seq": 5023,
  "until_seq": 5159,
  "reason": "ring_buffer_overflow"
}
```

| Field | Meaning |
|-------|---------|
| `count` | How many `seq` values were lost |
| `since_seq` | Last seq successfully held before the drop |
| `until_seq` | First seq held after the drop |
| `reason` | `ring_buffer_overflow` (V1 the only value) |

The marker itself is assigned its own `seq` (one past `until_seq`'s
predecessor) and travels through the normal ingest path. The hub
materializes it as a row in the events table; the Web UI uses it to
render a "gap of N events" visual.

### 5.6 Forward-compat: unknown variants

A hub receiving an unknown `kind` must:

1. Store the line verbatim in the events table with
   `payload_raw = <original JSON>`, `parse_status = "unknown_kind"`.
2. **Not** error the batch. Return `202` with the row included in the
   accepted count.
3. Surface the row in the Web UI as `(unknown event: kind="x.y")` so
   operators see new things rather than missing them.

This is the only forward-compat mechanism in V1; per-field forward-
compat relies on serde `#[serde(default)]` conventions in the protocol
crate.

---

## 6. Aggregation state machine (the connector side)

This is the algorithm the connector runs to project `CoreEvent` →
`EventEnvelope`. It is small but precise — small enough to fit on one
page; precise enough to write tests against.

### 6.1 State

Per `(instance_id, session_id, turn_id)`:

```
struct TurnAggregator {
    turn_id: String,
    next_block_index: u32,
    // currently-open text or thinking block, if any:
    open: Option<OpenBlock>,
}

struct OpenBlock {
    kind: TextOrThinking,          // text | thinking
    item_id: String,               // the Protocol ItemStarted's id
    block_index: u32,
    started_at: DateTime<Utc>,
    buffer: String,                // raw concatenated deltas
    agent_id: Option<AgentId>,
}
```

Per session: a monotonic `seq` counter.
Global: the in-memory ring of pending `EventEnvelope`s.

### 6.2 Transitions

Triggered on each `CoreEvent` observed via the subscriber sink:

| Input event | Action |
|-------------|--------|
| `Protocol(TurnStarted { turn_id })` | Allocate a `TurnAggregator { turn_id, next_block_index: 0, open: None }`. Forward the event as `kind: "protocol"`. |
| `Protocol(ItemStarted { item_id, kind: text \| thinking, agent_id })` | If `open` is `Some`, flush it first (defensive). Set `open = OpenBlock { kind, item_id, block_index: agg.next_block_index, buffer: "", agent_id, started_at: now }`; `agg.next_block_index += 1`. Forward `ItemStarted` itself as `kind: "protocol"`. |
| `Stream(TextDelta { delta, … })` | If `open.kind == text`, `open.buffer.push_str(&delta)`. Otherwise log + drop (shouldn't happen). **Do not** forward the delta to the wire. |
| `Stream(ThinkingDelta { delta, … })` | Same, for `open.kind == thinking`. |
| `Protocol(ItemCompleted { item_id })` | If `open.item_id == item_id`, emit one `TextBlockCompleted` / `ThinkingBlockCompleted` carrying `open.buffer` + metadata; set `open = None`. Forward `ItemCompleted` itself as `kind: "protocol"`. |
| `Protocol(ItemUpdated { item_id, .. })` for non-text items (e.g. tool item summary) | Forward as `kind: "protocol"`. Do not touch the aggregator buffer. |
| `Stream(ToolUseQueued / Started / Completed)` | Forward 1:1 as the corresponding `kind`. Do not touch the aggregator buffer. (Tool items live in their own `block_index` space and are interleaved naturally via `seq` ordering.) |
| `Stream(McpToolCallBegin / End)` | Forward 1:1 as `kind: "mcp_tool_call_*"`. |
| `Protocol(TurnCompleted / TurnFailed / TurnInterrupted { turn_id })` | If aggregator still has an `open` block (e.g. the agent crashed mid-stream and `ItemCompleted` never came), flush it with `completed_at = now` and a `was_truncated: true` field. Drop the `TurnAggregator`. Forward the terminating Protocol event last. |
| `Tui(_)` | Drop silently. Do not even count toward `seq`. |
| Any other `Protocol(_)` | Forward as `kind: "protocol"`. |

### 6.3 What this guarantees

- One `TextBlockCompleted` per text content block; multiple blocks per
  turn are common (`block_index` 0, 1, 2 …).
- Tool calls appear as their own events in the timeline, interleaved
  with text blocks via the shared `seq` ordering.
- Per-token deltas never reach the wire.
- A mid-turn crash flushes whatever was accumulated as `was_truncated:
  true`, so the hub doesn't silently lose text.
- The `seq` ordering between aggregated blocks, raw tool events, and
  pass-through Protocol events is what the hub uses to reconstruct the
  visual timeline — wall-clock `ts` is for display only.

### 6.4 What this does NOT guarantee (and why it's OK)

- **Real-time view of in-progress text.** By design (R3.1). The TUI
  remains the live surface.
- **Recovery of text from a connector-crashed buffer.** The buffer is
  in-memory only (D1). If the agent process dies, the in-flight
  `OpenBlock` is gone. The session JSONL on disk retains the message
  the agent committed if/when it committed.
- **Exact byte-identical reproduction of the model's stream.** The
  aggregator concatenates UTF-8 deltas; if the stream had soft
  boundaries (carriage-return rewrites etc.) the hub view may differ
  from the TUI view. This is acceptable for the analytical surface.

---

## 7. Ingest mechanics

### 7.1 Batching strategy

The connector accumulates envelopes in its ring buffer (sized by
`event_hub_ring_buffer_size`, default 10 000) and emits a `batch`
frame when **any** of the following holds:

- Pending envelopes ≥ `event_hub_batch_max_events` (default 1000).
- Pending serialized bytes would exceed `event_hub_batch_max_bytes`
  (default 1 MiB).
- ≥ `event_hub_batch_max_interval_ms` since the last successful send
  (default 500 ms).
- Ring buffer ≥ 80 % full (early flush to defer overflow).
- The WS connection has just been (re)established and there is a
  pending backlog (drain immediately).
- On shutdown: one final synchronous flush attempt with a 2 s deadline.

There is one in-flight WS frame at a time per connection (the WS write
half is single-task). New envelopes accumulate while a frame is in
flight; they go in the next batch.

### 7.2 Reconnect policy

There is **one WS connection** per `coco` process for the lifetime of
the process. Any of the following counts as a broken connection:

- WS close frame received (any code).
- Write error / read error on the underlying socket.
- TCP RST or TLS error.
- The WS handshake itself fails on a fresh attempt.

On a broken connection the connector:

1. Marks state `reconnecting` (visible via `tracing` / metrics — see
   §11).
2. Returns any in-flight `batch` envelopes to the **front** of the
   ring buffer (preserves seq order).
3. Sleeps for exponential backoff: `100ms → 200ms → 400ms → … → 30s`
   cap, with ±20 % jitter.
4. Opens a new WS to `event_hub_url`.
5. Sends `announce` (§3.1); the hub uses `instance_id` to recognize
   this as a reconnect rather than a new instance.
6. On `announce_ack`, resumes draining the ring buffer from the oldest
   `seq` outward.

If the ring buffer fills during the reconnect window, **oldest
envelopes drop first** and an `EventsDropped` marker is enqueued for
the affected range. The marker is delivered through the normal flow
once the connection is back.

A handshake that fails with WS close code `4000`–`4002` (bad request /
version mismatch / auth failure) is treated as **non-retriable on the
same config** — the connector logs and waits for reconfiguration
instead of hot-looping.

### 7.3 Ordering & dedup

- The hub keys events by `(instance_id, session_id, seq)`; resent
  envelopes after a reconnect are silent no-ops.
- The connector never re-orders: within one WS connection, frames
  carry envelopes in non-decreasing `seq` order. TCP guarantees the
  byte order within one connection.
- Across a reconnect, the connector may resend already-persisted
  envelopes; hub-side dedup keeps the storage exactly-once.

### 7.4 Concurrency

The connector runs three tasks:

- **Aggregator task** — consumes the `mpsc::Receiver<CoreEvent>` from
  `QueryEngine`, runs the §6 state machine, assigns `seq`, and pushes
  envelopes into the ring buffer.
- **Sender task** — owns the WS write half; pulls envelopes from the
  ring buffer, batches per §7.1, sends frames.
- **Reader task** — owns the WS read half; consumes
  `announce_ack` / `batch_ack` / `error` / future control frames.
  Surfaces close-frame information to the sender for reconnect.

Per-session `seq` is assigned by the aggregator only, so there is no
cross-task race on `seq`.

---

## 8. Configuration surface

### 8.1 Settings keys

Flat keys at the root of `settings.json`, all prefixed `event_hub_`.
**Presence of `event_hub_url` is the single source of truth for
"connector enabled"** — there is no separate `enabled` flag. When
`event_hub_url` is `null` (the default), the connector subsystem is
not spawned and the hot path pays zero cost.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `event_hub_url` | `string \| null` | `null` | `ws://host:port/v1/connect` or `wss://…`. `null` ⇒ disabled. |
| `event_hub_bearer_token` | `string \| null` | `null` | **Reserved field, V1 ignores its value.** Round 4 deferred auth out of V1 entirely; this key stays in the schema so adding auth later is non-breaking. |
| `event_hub_ring_buffer_size` | `u32` | `10000` | Max envelopes held in memory before drop-oldest |
| `event_hub_batch_max_events` | `u32` | `1000` | Max envelopes per `batch` frame |
| `event_hub_batch_max_bytes` | `u32` | `1048576` | Max serialized bytes per WS frame (1 MiB) |
| `event_hub_batch_max_interval_ms` | `u32` | `500` | Max time pending envelopes wait before flush |

### 8.2 Environment variables

Mapped 1 : 1 via `coco_config::EnvKey` (per CLAUDE.md root convention:
all coco-owned env vars use `COCO_*` and are registered in `EnvKey`,
never read ad-hoc inside a crate):

| Env var | Maps to |
|---------|---------|
| `COCO_EVENT_HUB_URL` | `event_hub_url` |
| `COCO_EVENT_HUB_BEARER_TOKEN` | `event_hub_bearer_token` |
| `COCO_EVENT_HUB_RING_BUFFER_SIZE` | `event_hub_ring_buffer_size` |
| `COCO_EVENT_HUB_BATCH_MAX_EVENTS` | `event_hub_batch_max_events` |
| `COCO_EVENT_HUB_BATCH_MAX_BYTES` | `event_hub_batch_max_bytes` |
| `COCO_EVENT_HUB_BATCH_MAX_INTERVAL_MS` | `event_hub_batch_max_interval_ms` |

### 8.3 CLI flags

| Flag | Effect |
|------|--------|
| `--event-hub-url <URL>` | Overrides `event_hub_url`. URL must be `ws://` or `wss://`. |
| `--serve-hub` | All-in-one mode: spawns the hub server in the same process on `--hub-port` (default 8731) and auto-sets `event_hub_url = ws://127.0.0.1:<port>/v1/connect` (unless an explicit URL is already configured) |
| `--hub-port <N>` | Port for `--serve-hub` (default 8731) |

The same port serves both the WS endpoint and the read-side HTTP query
endpoints + Web UI.

### 8.4 All-in-one mode behavior

With `--serve-hub`, the agent process:

1. Spawns the hub server on `--hub-port` in the same Tokio runtime.
2. Sets `event_hub_url` to `ws://127.0.0.1:<port>/v1/connect` if not
   already configured.
3. Serves WS, HTTP query endpoints, and the bundled Web UI from the
   single port.

Loopback binding is the default; the hub refuses to bind a non-loopback
address until `event_hub_bearer_token` is configured (round-5 auth
clause, surfaced here only to prevent footguns).

---

## 9. Error semantics

### 9.1 WS close codes (hub → client)

The hub's authoritative signal for "what went wrong" is the WS close
code. Standard codes `1000-1011` keep their RFC 6455 meanings;
application-specific codes use the `4000-4999` range:

| Code | Meaning | Connector action |
|------|---------|------------------|
| `1000` Normal Closure | Hub shutting down gracefully | Reconnect with backoff |
| `1001` Going Away | Hub restart (e.g. config reload) | Reconnect with backoff |
| `1011` Internal Error | Hub bug | Reconnect with backoff |
| `4000` Bad Request | Malformed announce / frame | Log, **do not retry** on same config |
| `4001` Protocol Version Mismatch | No `Sec-WebSocket-Protocol` overlap | Log, **do not retry** until reconfigured |
| `4002` Auth Failed | Bearer token invalid / missing — **reserved, not emitted by V1 hub** | Log, **do not retry** until reconfigured |
| `4013` Frame Too Large | Frame exceeded hub limit | Log, **do not retry** (configuration error) |
| `4029` Rate Limited | Hub backpressure | Reconnect with **longer** backoff |

WS close codes ≥ 4000 are application-defined (RFC 6455 §7.4.2). We
deliberately stay away from the standard 1002/1003/1007/1008 codes —
their wire semantics are too overloaded.

### 9.2 The `error` frame (hub → client)

Before closing with a `4xxx` code, the hub sends one diagnostic frame:

```json
{
  "kind": "error",
  "code": "bad_request"
        | "protocol_version_mismatch"
        | "auth_failed"
        | "frame_too_large"
        | "rate_limited",
  "detail": "human-readable explanation"
}
```

The close code is the **authoritative** signal for the connector's
retry decision; the `error` frame is for human diagnostics (and is
forwarded to `tracing`).

### 9.3 HTTP query endpoint errors

For the read-side `GET /v1/instances/*` and `GET /v1/search`:

| Code | Meaning |
|------|---------|
| 200 | Success |
| 400 | Bad query parameter |
| 404 | Instance / session not found |
| 5xx | Hub-side fault |

These follow standard REST conventions; the connector does not consume
these endpoints, so retry policy is left to clients (Web UI, external
tools).

---

## 10. What is intentionally **not** specified here

These come in later rounds; specifying them now would bloat the contract
without clarifying the V1 wire:

- **Storage schema** (round 4) — SQLite tables, FTS5 indexing, retention
- **Search query language** (round 4) — `GET /v1/search` params
- **Auth model** (round 5) — bearer token, mTLS, loopback exemption
- **Web UI** (round 5) — SPA structure, routes
- **Phase-3 control plane** (parked) — WebSocket frame format, control
  message taxonomy
- **OTLP export** (post-V1) — translating EventEnvelope → OTLP spans
- **Tool-output truncation policy** (round 4) — blob refs vs inline

Any decision in those documents **must** be compatible with the contract
above; they extend it, they don't redefine it.

---

## 11. Crate touch list (preview, full layout is round 5)

Roughly:

> **Path / crate names finalized in round 4 §10** — three crates
> grouped under `coco-rs/hub/`. The preview here is just the round-3
> wire-protocol contribution to that layout.

- **NEW** `coco-rs/hub/protocol/` (`coco-hub-protocol`) — pure wire
  types: `EventEnvelope`, `EventPayload`, `AnnounceFrame`,
  `AnnounceAckFrame`, `BatchFrame`, `BatchAckFrame`, `ErrorFrame`,
  close-code enum. Zero coco-rs internal deps; both connector and
  server depend on it.
- **NEW** `coco-rs/hub/connector/` (`coco-hub-connector`) — agent-side:
  aggregator state machine, ring buffer, batcher, **WebSocket client
  (via `tokio-tungstenite`)**, three-task layout (aggregator / sender /
  reader, §7.4). Depends on `coco-hub-protocol`, `coco-types`,
  `utils/secret-redact`, `common/otel`. **Linked unconditionally** into
  `coco-rs/app/cli`.
- **NEW** `coco-rs/hub/server/` (`coco-hub-server`) — hub-side binary +
  library: WS ingest, HTTP API, Web UI, SSE, retention. Storage trait
  and SQLite impl are modules inside this crate (round 4 §10.1).
  **Optional dependency** of `coco-rs/app/cli` behind a `serve-hub`
  Cargo feature; **`default = []`** so a plain `cargo build` of
  `coco` does not include the hub server (no axum / sqlite / askama /
  Tailwind asset build). Opt-in with `--features serve-hub` or the
  `just coco-with-hub` recipe (round 4 §10.3).
- **CHANGED** `coco-rs/app/session/src/concurrent_sessions.rs` — add
  one `instance_id: Uuid` field to `SessionRegistration`.
- **CHANGED** `coco-rs/app/query/src/engine.rs` (or wherever the
  `CoreEvent` sender lives) — clone an additional
  `Sender<CoreEvent>` for the connector when `event_hub_url` is set.
- **CHANGED** `coco-rs/app/cli/src/` — wire `--event-hub-url` /
  `--serve-hub` / `--hub-port` flags into bootstrap. `--serve-hub`
  is feature-gated.
- **CHANGED** `coco-rs/common/config/src/` — add the six
  `event_hub_*` keys to `Settings`, register the six `COCO_EVENT_HUB_*`
  env keys in `EnvKey`.

---

## 12. Open items for round 4

Round 4 (`04-hub-storage-and-search.md`) needs to specify:

- SQLite schema (tables: `instances`, `sessions`, `events`,
  `agent_edges`, optional `events_raw`)
- FTS5 indexing strategy: full-text over which fields?
  (`text_block_completed.full_text` + `tool_use_completed.output_text` +
  Protocol message bodies?)
- Retention: TTL + size cap; defaults; per-instance overrides.
- Sub-agent edge materialization rules (re-derived at ingest from
  `SubagentSpawned` Protocol events).
- Tool-output handling for huge payloads (inline vs blob ref).
- `GET /v1/search` query parameter set.
- Indexing strategy for cold storage (out of scope V1, just reserve
  the migration path).
