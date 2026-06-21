# Round 4 — Hub Storage, Search, and Web UI

> Status: **draft contract** for the hub side.
> Depends on: `01..03`.
> Round-3 fixed the wire (what arrives at the hub); round 4 fixes
> *what the hub does with it* — schema, indexes, search API, UI stack.

This document pins down three coupled concerns at the same time because
they constrain each other:

1. **Storage schema** — which columns exist (and therefore which can be
   indexed).
2. **Search API** — what queries the schema supports, exposed as HTTP.
3. **Web UI** — what the operator can do with that API, rendered by
   Axum + Askama + HTMX + Tailwind + Flowbite.

Anything not necessary for V1 (auth, federation, OTLP export, advanced
analytics) is explicitly out of scope.

---

## 1. Decisions inherited

| From | Decision | How it shows up here |
|------|----------|----------------------|
| D1 | No agent-side DB | Hub is the only durable store |
| D2 | Identity = opaque `instance_id`; rest are attributes | `instances` table separates identity from attribute columns |
| D3 | Hub = TUI-in-browser + search | The schema is a *projection* shaped for search/display, not session truth |
| R3.1 | Per-turn aggregation | `text_block_completed` is one row per content block |
| Round 3 | WS transport, NDJSON-style frames inside WS | Ingest is "decode WS frame → insert rows transactionally" |
| Round 4 | **Storage backend = SQLite** with indexes on fixed fields; **FTS5 / free-text search deferred to V2** | §2, §3 |
| Round 4 | **`EventStore` trait abstracts storage** so SQLite can be swapped later | §2.4 |
| Round 4 | **Web UI must work on desktop, mobile, and a future native app shell**; the Axum + Askama + HTMX + Tailwind + Flowbite + Prism stack was chosen precisely for this | §6 |
| Round 4 | **All hub-related crates live under `coco-rs/hub/`** (new top-level group); **3 crates: `coco-hub-protocol` + `coco-hub-connector` + `coco-hub-server`**; cli's dep on the server is feature-gated | §10 |
| Round 4 | **Retention defaults: 3 days, 3 GiB** | §8 |
| Round 4 | **V1 ships without auth.** No bearer-token check, no mTLS, no loopback-exemption logic — the hub accepts every connection on its bind address. Auth gets a dedicated future round. | §8.4, §9, §11 |
| Round 4 | **Tailwind v4 (current stable).** Latest stable CSS-first config — no `tailwind.config.js`; classes scanned from Askama templates. | §6.2 |
| Round 4 | **Web UI is desktop-first, adaptive down to mobile.** Design baseline is desktop; the same templates render usably on phones via responsive utilities. | §6.1.1 |

The HTMX/Askama choice is load-bearing. It means:

- **No npm in the Rust workflow.** The hub binary embeds the built
  CSS/JS via `include_dir!`; the only build-time external is the
  standalone `tailwindcss` CLI.
- **No client-side router.** Every page is server-rendered HTML;
  HTMX provides interactivity via `hx-get` / `hx-swap` and SSE for
  live updates.
- **No JSON-rendering layer.** The search/query handlers return HTML
  fragments for the UI; a parallel `/v1/...` JSON API exists for
  external consumers.

---

## 2. Storage choice — SQLite

> V1 stores everything in plain SQLite with **fixed-field indexes only**.
> FTS5 / full-text search is **deferred** — the user's call in round 4:
> *"第一阶段不用支持 FTS5，只需要能固定索引就行"*. The trait surface
> (§2.4) leaves room to add FTS later without touching call sites.

### 2.1 Why SQLite (not Postgres / DuckDB / RocksDB)

| Candidate | Verdict | Reason |
|-----------|---------|--------|
| **SQLite** | ✅ chosen | Single-file embedded; matches the all-in-one mode; mature `rusqlite` bindings; the `codex-rs` reference uses it (`state_5.sqlite`). FTS5 is available *when we want it* later. |
| Postgres | ❌ | Requires external service; defeats the "single binary" all-in-one promise. Future federation could federate to Postgres; not V1. |
| DuckDB | ⚠️ | Excellent for OLAP queries but the read/write concurrency model is awkward for a write-mostly ingest workload + interactive read UI. Reserve for a future "analytics warehouse" mode. |
| RocksDB / sled | ❌ | Key-value only; no indexed search at all. |

### 2.2 SQLite operational shape

- One database file: `<hub_data_dir>/events.sqlite`.
- `<hub_data_dir>` default: `~/.coco/hub/` for embedded mode,
  `$PWD/data/` for standalone.
- **WAL mode** for concurrent reads while ingest writes.
- `synchronous=NORMAL` (WAL gives durability; `FULL` is overkill for
  this workload — losing the last few ms on a hard crash is acceptable).
- `journal_size_limit=67108864` (64 MiB) so WAL doesn't grow unbounded.
- `mmap_size=268435456` (256 MiB) for read performance.
- Single writer at a time (SQLite serializes writes); reads concurrent.

### 2.3 Read/write task split

- **One writer task** owns the `rusqlite::Connection` in write mode and
  consumes a `mpsc::Receiver<IngestCommand>` from the WS reader. Every
  WS `batch` frame is one transaction.
- **Reader pool** for HTTP query handlers (Axum extracts `Arc<Pool>`).
  Use `deadpool-sqlite` for a small pool (e.g. 8 connections).

This split mirrors the codex-rs `state` runtime pattern. Mixing
ingestion writes with HTTP read traffic on one connection would
deadlock under load.

### 2.4 `EventStore` — storage interface abstraction

> User decision, round 4: *"访问存储需要提供接口抽象，后面用于切换存储"*.

All storage access goes through an `async` trait so the SQLite backend
can be swapped (Postgres, DuckDB, an external service) without
disturbing the hub server, ingest handlers, web/JSON routes, or the
retention sweep.

```rust
#[async_trait::async_trait]
pub trait EventStore: Send + Sync + 'static {
    // ── Ingest path ────────────────────────────────────────────────
    async fn upsert_instance(
        &self,
        announce: &AnnounceFrame,
    ) -> Result<UpsertInstanceOutcome, EventStoreError>;

    async fn mark_instance_gone(
        &self,
        instance_id: &str,
        reason: GoneReason,
    ) -> Result<(), EventStoreError>;

    async fn ingest_batch(
        &self,
        instance_id: &str,
        batch: BatchFrame,
    ) -> Result<IngestStats, EventStoreError>;

    // ── Query: instances ──────────────────────────────────────────
    async fn list_instances(
        &self,
        params: ListInstancesParams,
    ) -> Result<Page<InstanceRow>, EventStoreError>;

    async fn get_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRow>, EventStoreError>;

    // ── Query: sessions ───────────────────────────────────────────
    async fn list_sessions(
        &self,
        instance_id: &str,
        params: ListSessionsParams,
    ) -> Result<Page<SessionRow>, EventStoreError>;

    async fn get_session(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, EventStoreError>;

    // ── Query: events ─────────────────────────────────────────────
    async fn list_events(
        &self,
        query: EventQuery,
    ) -> Result<Page<EventRow>, EventStoreError>;

    async fn get_event(
        &self,
        instance_id: &str,
        session_id: &str,
        seq: i64,
    ) -> Result<Option<EventRow>, EventStoreError>;

    // ── Search ────────────────────────────────────────────────────
    async fn search(
        &self,
        query: SearchQuery,
    ) -> Result<Page<SearchHit>, EventStoreError>;

    // ── Agent topology ────────────────────────────────────────────
    async fn list_agent_edges(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Vec<AgentEdge>, EventStoreError>;

    // ── Maintenance ───────────────────────────────────────────────
    async fn run_retention_sweep(
        &self,
        policy: &RetentionPolicy,
    ) -> Result<SweepStats, EventStoreError>;

    async fn health(&self) -> Result<HealthSnapshot, EventStoreError>;
}
```

**Boundary rules:**

- **The hub server takes `Arc<dyn EventStore>` everywhere.** No direct
  `rusqlite::Connection` reference outside the SQLite impl.
- **Return types are backend-agnostic.** `InstanceRow`, `SessionRow`,
  `EventRow`, etc. are plain structs owned by this crate; they are not
  `rusqlite::Row` projections.
- **Cursors are opaque.** `Page<T> { items: Vec<T>, next_cursor:
  Option<Cursor> }` where `Cursor = String` (base64 by convention).
  Each impl encodes its own cursor format; the hub doesn't inspect it.
- **`SearchQuery::q` is reserved for the future free-text channel.**
  The V1 `SqliteEventStore` impl returns
  `EventStoreError::NotSupported("free-text search not enabled in this
  build")` if `q` is `Some`. The hub's JSON API rejects requests that
  set `q` (HTTP 400) in V1, and the Web UI does not render a free-text
  input. Once FTS5 (or another text engine) lands, the field is in
  place — no protocol change, no UI restructure.
- **Ingest is *not* responsible for fanning out to live SSE
  subscribers.** That stays in-process inside the hub server (a
  `broadcast::Sender<EventEnvelope>` per session topic — see §6.5).
  The trait is purely about durable storage.

**Trait-supporting types** (also owned by the store crate):

```rust
pub struct UpsertInstanceOutcome {
    pub first_seen: bool,
    pub previous_last_seen_at: Option<i64>,
}

pub enum GoneReason { GracefulClose, Reset, Timeout }

pub struct IngestStats {
    pub accepted: usize,
    pub duplicates: usize,
    pub parse_failures: usize,
}

pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<Cursor>,
    pub estimated_total: Option<i64>,
}
pub type Cursor = String;

pub struct EventQuery {
    pub instance_id: String,
    pub session_id: Option<String>,
    pub before: Option<Cursor>,
    pub limit: u32,
    pub filter: EventFilter,
}

pub struct SearchQuery {
    pub q: Option<String>,
    pub filter: EventFilter,
    pub before: Option<Cursor>,
    pub limit: u32,
}

pub struct EventFilter {
    pub kind: Option<Vec<String>>,
    pub inner_kind: Option<Vec<String>>,
    pub tool_name: Option<Vec<String>>,
    pub agent_id: Option<String>,
    pub is_error: Option<bool>,
    pub from: Option<i64>,   // unix ms
    pub to: Option<i64>,
}

pub struct RetentionPolicy {
    pub max_age_ms: i64,
    pub max_total_bytes: i64,
    pub vacuum_after_freed_bytes: i64,
}
```

**Module layout** (lives inside `coco-hub-server`; see §10 for the
three-crate decision):

```
coco-hub-server/src/store/
├── mod.rs           ── pub trait EventStore + re-exports
├── model.rs         ── InstanceRow, SessionRow, EventRow, AgentEdge, SearchHit
├── query.rs         ── EventQuery, SearchQuery, EventFilter, Page, Cursor
├── error.rs         ── EventStoreError (snafu, ErrorExt)
├── retention.rs     ── RetentionPolicy, SweepStats
├── mock.rs          ── feature = "test-util" — MockEventStore
└── sqlite/          ── feature = "sqlite" (default)
    ├── mod.rs           ── SqliteEventStore
    ├── schema.rs        ── DDL + migrations (§3)
    ├── ingest.rs        ── impl of ingest_batch (§4)
    ├── search.rs        ── structured-filter compiler (§7.1)
    ├── retention.rs     ── sweep + vacuum (§8)
    └── cursor.rs        ── (seq, ts) ↔ base64 round-trip
```

The trait + impls do **not** get their own crate — they're a module
inside `coco-hub-server`. This keeps the workspace at three hub
crates (per user decision in round 4) while still preserving the
swap-storage-later capability via the trait + Cargo features inside
the server crate.

`SqliteEventStore::new(path: PathBuf) -> Result<Self, _>` constructs
the connection pool, applies migrations, starts the writer task. The
server holds one `Arc<dyn EventStore>`; behind that arc may sit
`SqliteEventStore` today, `PgEventStore` tomorrow. The future Postgres
impl lives under `src/store/postgres/` behind `feature = "postgres"`
— still no new crate.

**Test doubles:** `MockEventStore` ships under `#[cfg(any(test,
feature = "test-util"))]` so hub-server unit tests don't need a real
SQLite file.

---

## 3. Schema

> The DDL below is the **V1 SQLite expression** of the data model the
> `EventStore` trait exposes. A future Postgres / DuckDB impl will map
> the same `InstanceRow` / `SessionRow` / `EventRow` types onto its own
> equivalent tables (the trait's row types are the contract; the SQL
> is the SQLite implementation of it).

### 3.1 Tables

```sql
-- 3.1.a  Instances: one row per coco process lifetime.
CREATE TABLE instances (
    instance_id    TEXT PRIMARY KEY,        -- UUID v4 (announced)
    host           TEXT NOT NULL,
    cwd            TEXT NOT NULL,
    pid            INTEGER NOT NULL,
    started_at     INTEGER NOT NULL,         -- unix ms (from announce)
    version        TEXT NOT NULL,
    kind           TEXT NOT NULL,            -- 'interactive' | 'bg' | 'daemon' | 'daemon_worker'
    entrypoint     TEXT,
    name           TEXT,
    first_seen_at  INTEGER NOT NULL,         -- unix ms (hub clock, on first announce)
    last_seen_at   INTEGER NOT NULL,         -- unix ms (hub clock, on every batch)
    status         TEXT NOT NULL DEFAULT 'unknown'  -- 'live' | 'gone' | 'unknown'
);

-- 3.1.b  Sessions: derived from the event stream (no separate announce).
CREATE TABLE sessions (
    instance_id    TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    started_at     INTEGER,                  -- from SessionStarted, may be NULL initially
    ended_at       INTEGER,                  -- from SessionEnded
    model          TEXT,                     -- last observed model
    total_input_tokens   INTEGER DEFAULT 0,
    total_output_tokens  INTEGER DEFAULT 0,
    total_cost_usd       REAL    DEFAULT 0,
    last_seq       INTEGER DEFAULT 0,
    last_event_ts  INTEGER,
    discovered_via TEXT NOT NULL DEFAULT 'session_started',
                                              -- 'session_started' | 'out_of_band'
    PRIMARY KEY (instance_id, session_id),
    FOREIGN KEY (instance_id) REFERENCES instances(instance_id) ON DELETE CASCADE
);

-- 3.1.c  Events: one row per envelope.
CREATE TABLE events (
    instance_id    TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    seq            INTEGER NOT NULL,
    ts             INTEGER NOT NULL,         -- unix ms (envelope.ts; agent clock)
    received_at    INTEGER NOT NULL,         -- unix ms (hub clock)
    schema_version INTEGER NOT NULL,
    kind           TEXT NOT NULL,            -- 'protocol' | 'tool_use_*' | 'text_block_completed' | ...

    -- Denormalized fields for indexed search.
    -- All NULL when not applicable; extracted at ingest time.
    turn_id        TEXT,
    agent_id       TEXT,
    item_id        TEXT,
    tool_name      TEXT,
    call_id        TEXT,
    is_error       INTEGER,                  -- 0 | 1 | NULL
    inner_kind     TEXT,                     -- e.g. 'turn_started' inside 'protocol'

    -- Raw envelope.
    payload        TEXT NOT NULL,            -- the full envelope JSON
    payload_size   INTEGER NOT NULL,
    parse_status   TEXT NOT NULL DEFAULT 'ok',
                                              -- 'ok' | 'partial' | 'failed' | 'unknown_kind'

    -- Tool-output overflow handling (§5).
    tool_output_truncated  INTEGER NOT NULL DEFAULT 0,
    tool_output_full_size  INTEGER,           -- original bytes when truncated

    -- UI preview (NOT indexed — see §3.4 + §4.2).
    preview        TEXT,

    PRIMARY KEY (instance_id, session_id, seq),
    FOREIGN KEY (instance_id, session_id)
        REFERENCES sessions(instance_id, session_id) ON DELETE CASCADE
);

-- 3.1.d  Sub-agent topology: re-derived from SubagentSpawned events.
CREATE TABLE agent_edges (
    instance_id      TEXT NOT NULL,
    session_id       TEXT NOT NULL,
    parent_agent_id  TEXT NOT NULL,
    child_agent_id   TEXT NOT NULL,
    agent_type       TEXT,
    spawned_at       INTEGER NOT NULL,
    completed_at     INTEGER,
    status           TEXT NOT NULL DEFAULT 'running',
                                              -- 'running' | 'idle' | 'completed' | 'failed' | 'backgrounded'
    PRIMARY KEY (instance_id, session_id, child_agent_id),
    FOREIGN KEY (instance_id, session_id)
        REFERENCES sessions(instance_id, session_id) ON DELETE CASCADE
);

-- 3.1.e  (V2) FTS5 free-text index — NOT created in V1.
-- Reserved here for documentation only; do not include this DDL in the
-- V1 migration. When FTS lands it will be added as a follow-up
-- migration that backfills text content from existing events.
```

### 3.2 Indexes

```sql
-- instances
CREATE INDEX idx_instances_host_cwd      ON instances(host, cwd);
CREATE INDEX idx_instances_last_seen     ON instances(last_seen_at DESC);
CREATE INDEX idx_instances_status        ON instances(status);

-- sessions
CREATE INDEX idx_sessions_started        ON sessions(started_at DESC);
CREATE INDEX idx_sessions_last_event     ON sessions(last_event_ts DESC);

-- events: the four high-value lookup paths
CREATE INDEX idx_events_session_ts       ON events(instance_id, session_id, ts);
CREATE INDEX idx_events_kind             ON events(kind);
CREATE INDEX idx_events_tool_name        ON events(tool_name)
                                          WHERE tool_name IS NOT NULL;
CREATE INDEX idx_events_errors           ON events(instance_id, session_id, seq)
                                          WHERE is_error = 1;
CREATE INDEX idx_events_received_at      ON events(received_at);
CREATE INDEX idx_events_agent            ON events(instance_id, session_id, agent_id)
                                          WHERE agent_id IS NOT NULL;

-- agent_edges
CREATE INDEX idx_agent_edges_parent      ON agent_edges(instance_id, session_id, parent_agent_id);
CREATE INDEX idx_agent_edges_status      ON agent_edges(status);
```

### 3.3 Indexable columns — the "fixed fields"

The user asked for indexes on fixed fields. These are the columns we
commit to indexing in V1:

| Column | Why indexed |
|--------|-------------|
| `instance_id` (PK component) | Primary navigation |
| `session_id` (PK component) | Primary navigation |
| `seq` (PK component) | Ordering inside a session |
| `ts` (composite with instance/session) | Timeline queries |
| `kind` | Filter by event type |
| `tool_name` (partial) | "Show me all `Shell` calls" |
| `agent_id` (partial) | "Show me sub-agent X's activity" |
| `is_error` (partial) | "Show me all failures" |
| `received_at` | Retention sweeps |
| `host`, `cwd` (on instances) | Group by project / machine |
| `last_seen_at` (on instances) | Sort instance list by recency |
| `last_event_ts` (on sessions) | Sort sessions by recency |
| `parent_agent_id` (on agent_edges) | Sub-agent tree walks |

Adding a new indexed dimension later is one `CREATE INDEX` away;
intentionally we don't pre-create indexes "in case" — they cost write
throughput.

### 3.4 Free-text search — deferred to V2

V1 ships **no full-text index**. All searches are over the structured
columns listed in §3.3. The ingest pipeline still extracts a short
*preview* string from text-bearing payloads (§4.2) and stores it on
the row so the UI can render snippets without re-parsing JSON, but
this preview is **not indexed** and not searchable.

When V2 adds full-text (most likely FTS5, but the trait is impl-
agnostic), it will be a single follow-up migration that backfills text
from existing rows; the existing schema doesn't need restructuring.

---

## 4. Ingest pipeline

### 4.1 Per-frame transaction (SQLite impl of `EventStore::ingest_batch`)

For each WS `batch` frame received, the `SqliteEventStore` writer task
runs one transaction:

```rust
// SqliteEventStore::ingest_batch — internal flow.
async fn ingest_batch_inner(
    writer: &mut Connection,
    instance_id: &str,
    batch: BatchFrame,
) -> Result<IngestStats, EventStoreError> {
    let tx = writer.transaction()?;
    let mut stats = IngestStats::default();

    for envelope in batch.events {
        let denorm = denormalize(&envelope);   // §4.2

        match insert_event_row(&tx, &envelope, &denorm) {
            Ok(()) => stats.accepted += 1,
            Err(e) if is_pk_violation(&e) => stats.duplicates += 1,
            Err(e) => return Err(e.into()),
        }

        update_session_rollup(&tx, &envelope, &denorm)?;
        update_agent_edges_if_needed(&tx, &envelope, &denorm)?;
        update_instance_last_seen(&tx, instance_id)?;
    }

    tx.commit()?;
    Ok(stats)
}
```

One transaction per frame keeps write throughput high (no per-row
fsync) and gives natural at-least-once semantics across reconnects
(duplicate `(instance, session, seq)` is silently counted as a
duplicate). The fan-out to live SSE subscribers happens **outside this
function** in the hub server's `IngestPipeline` — see §6.5.

> When V2 adds full-text search, an additional
> `insert_fts_row(&tx, …)` call slots in here. The preview string is
> already extracted by `denormalize()` (§4.2), so the migration is
> additive only.

### 4.2 Denormalization rules

The ingest pipeline pulls the following from each `EventEnvelope`:

| Payload `kind` | `inner_kind` | `turn_id` | `agent_id` | `item_id` | `tool_name` | `call_id` | `is_error` | `preview` (UI snippet, **NOT indexed**) |
|---|---|---|---|---|---|---|---|---|
| `protocol` (`turn_started`) | `turn_started` | from `notification.turn_id` | — | — | — | — | — | — |
| `protocol` (`turn_completed`) | `turn_completed` | yes | — | — | — | — | from `notification` | — |
| `protocol` (`subagent_spawned`) | `subagent_spawned` | — | child id | — | — | — | — | first 200 chars of `description` |
| `protocol` (`item_*`) | matching | — | yes if present | yes | — | — | — | — |
| `protocol` (`error`) | `error` | maybe | maybe | — | — | — | `1` | first 200 chars of error message |
| `protocol` (other) | inner type | best effort | best effort | best effort | — | — | best effort | — |
| `tool_use_queued` | — | — | yes | — | `tool_name` | `call_id` | — | first 200 chars of tool input (JSON) |
| `tool_use_started` | — | — | yes | — | `tool_name` | `call_id` | — | — |
| `tool_use_completed` | — | — | yes | — | `tool_name` | `call_id` | `is_error` | first 200 chars of tool output |
| `mcp_tool_call_begin` | — | — | yes | — | server name + tool | call id | — | first 200 chars of input |
| `mcp_tool_call_end` | — | — | yes | — | server name + tool | call id | `is_error` | first 200 chars of output |
| `text_block_completed` | — | yes | yes | — | — | — | — | first 200 chars of `full_text` |
| `thinking_block_completed` | — | yes | yes | — | — | — | — | first 200 chars of `full_text` |
| `events_dropped` | — | — | — | — | — | — | — | — |

Missing fields stay `NULL`; partial-index `WHERE … IS NOT NULL`
clauses (§3.2) keep the indexes lean.

The `preview` column is **just for UI display** (so the events list
can show a snippet without parsing the `payload` JSON on every row).
It is *not* indexed. When V2 adds full-text search, the same
`preview` extraction (or a wider variant) becomes the FTS5
`text_content`.

### 4.3 Sub-agent edge materialization

On `protocol` payload with `inner_kind == "subagent_spawned"`:

```sql
INSERT INTO agent_edges (instance_id, session_id, parent_agent_id,
                         child_agent_id, agent_type, spawned_at, status)
VALUES (?, ?, ?, ?, ?, ?, 'running')
ON CONFLICT (instance_id, session_id, child_agent_id) DO NOTHING;
```

On `subagent_completed`, `subagent_backgrounded`, or final
`subagent_progress`:

```sql
UPDATE agent_edges
   SET status = ?, completed_at = ?
 WHERE instance_id = ? AND session_id = ? AND child_agent_id = ?;
```

This re-derivation is **idempotent**; a retried batch produces the
same edge state.

### 4.4 Backpressure

If the writer task's `mpsc::Receiver` queue is more than 1 000 batches
deep, the WS reader pauses reading until the queue drops below 500.
This propagates TCP-level backpressure to the agent — exactly the
behavior round 3 designed for.

---

## 5. Tool output handling

V1 keeps it simple:

- The connector sends tool outputs **inline** in the envelope,
  redacted via `utils/secret-redact`.
- The hub stores up to `hub_max_tool_output_inline` bytes
  (default **262 144 B = 256 KiB**) per tool-output field, after which
  the row records `tool_output_truncated = 1` and
  `tool_output_full_size = <bytes>`. Truncated content has an explicit
  marker in `payload` (e.g. `"output_text": "…[truncated 250 KiB]"`).
- WS frame hard ceiling at the hub is **10 MiB**
  (`hub_max_frame_bytes`). Frames larger than that trigger close code
  `4013 Frame Too Large` — the connector is misconfigured.
- No blob storage in V1. Full payloads remain at the agent's
  ephemeral state (and in `~/.coco/projects/<slug>/<session>.jsonl`).

Round 5 may add an out-of-band blob store; the row already has the
fields (`tool_output_full_size`) to point to it later.

---

## 6. Web UI architecture

### 6.1 Stack

> Round-4 user rationale: *"webui 要支持桌面端和移动端打开，未来要支持
> app，这个是采用 Axum + Askama + HTMX + Tailwind CSS + Flowbite 的
> 原因"* + later: *"页面是桌面端 first，但是要自适应大小支持移动端"*.
>
> Read together: **the operator's primary surface is a desktop browser**
> (that's where richness happens). Mobile is a fallback — the same
> pages must render usably on a phone, but design priority is desktop.
> A future native app shell wraps the same HTTP endpoints.

| Layer | Choice | Why this serves desktop primarily + mobile fallback + future app |
|-------|--------|------------------------------------------------|
| HTTP framework | **Axum** | One stack for WS ingest + HTTP query + HTML pages. Same endpoints a native shell can call. |
| Templates | **Askama** | Compile-time-checked HTML; no runtime template loader. |
| Interactivity | **HTMX** | Server-rendered HTML works in every browser, including any mobile WebView. No JS framework runtime. Future native app can embed a WebView and reuse pages, or call the JSON API and render natively. |
| Styling | **Tailwind CSS v4** | Utility-first with responsive breakpoints; CSS-first config (no `tailwind.config.js`); standalone CLI; no npm. |
| Components | **Flowbite** | Pre-styled Tailwind components — responsive variants (tables that scroll on narrow widths, drawers, dropdowns) are first-class. |
| Live updates | **Server-Sent Events** | Works in every modern browser, desktop and mobile. HTMX `hx-ext="sse"` handles wiring. |
| Syntax highlighting | **Prism.js** + 11 language bundles | See §6.2; ~120 KiB gzipped — fine even over mobile networks. |

Explicitly **not** using: React, Vue, Svelte, WASM frontend, esbuild,
Vite, npm. The Rust binary is the build artifact; a native shell wraps
the same HTTP/SSE/WS endpoints.

### 6.1.1 Responsive design — desktop-first, adaptive down

> User decision: *"页面是桌面端 first，但是要自适应大小支持移动端"*.

The visual design baseline is a desktop browser (≥ 1280 px). Phones
get an acceptable rendering of the same content via Tailwind's
responsive utilities; mobile is **not** a separately designed surface.

Concretely:

- **Design baseline = desktop.** Wireframes, density, information
  layout are decided for a desktop window first. Mobile is what the
  desktop layout reflows into.
- **Tailwind's mobile-first cascade is a tooling convenience, not a
  design statement.** In code we still write `class="grid grid-cols-3
  md:grid-cols-2 sm:grid-cols-1"` — the right-hand classes degrade for
  smaller screens. (Tailwind's defaults apply at the smallest breakpoint
  and `md:` / `lg:` add at wider; the *design* we're degrading from is
  the desktop one.)
- **Breakpoints we target:**
  - Desktop ≥ 1024 px (`lg:`): primary, full table layouts, sidebar
    visible.
  - Tablet 640–1024 px (`sm:` / `md:`): condensed tables, sidebar may
    collapse.
  - Phone < 640 px: single column, sidebar becomes an off-canvas drawer
    (Flowbite default).
- **No mobile-only features.** Anything reachable on mobile is also
  reachable (and likely more comfortable) on desktop. We don't ship
  swipe gestures, no pull-to-refresh, no native-feeling
  transitions — desktop wouldn't benefit and they cost design effort.
- **No client-side virtualization in V1.** Server-side cursor
  pagination via HTMX. Long lists scroll; users filter.

### 6.1.2 Future native app shell

The stack is forward-compatible with a native app in two stacks (we
don't pick one in V1, just don't preclude either):

- **WebView shell** (Tauri, Capacitor, plain iOS `WKWebView` /
  Android `WebView`): point at the hub's `/` and the same HTML pages
  render. SSE works in every modern WebView.
- **Native + JSON API**: a Swift/Kotlin/Flutter app calls the
  `/v1/*` JSON endpoints (§7) and renders its own UI. The JSON API
  already exists for external consumers — the native app is just
  another such consumer.

Either path works because the **HTTP surface is the contract**, not
the HTML. We treat the HTML as one consumer of that contract.

### 6.2 Browser-side dependencies (all served from hub binary)

| Asset | Source | Embedding |
|-------|--------|-----------|
| `htmx.min.js` | upstream HTMX 1.9.x | committed copy under `event-hub-server/web/static/` |
| `htmx-ext-sse.min.js` | HTMX extension | committed copy |
| `flowbite.min.js` | Flowbite 2.x | committed copy |
| `prism.min.js` | Prism.js 1.29.x (core) | committed copy |
| `prism-*.min.js` | Prism language bundles (see §6.2.1) | committed copies |
| `prism-tomorrow.min.css` | Prism dark theme (Flowbite-compatible) | committed copy |
| `prism-line-numbers.min.js` + `.css` | Prism plugin (optional) | committed copy |
| `style.css` | built from Tailwind input | `build.rs` invokes `tailwindcss` standalone CLI; output `include_bytes!`'d into the binary |
| favicon, fonts | static | committed |

**Tailwind version: v4 (current stable, CSS-first config).**

Tailwind v4 dropped the JS config file in favor of in-CSS directives
(`@theme`, `@plugin`, `@source`). The hub's `web/style.css` looks like:

```css
@import "tailwindcss";
@plugin "flowbite/plugin";
@source "../templates/**/*.html";
@source "../static/**/*.html";  /* if any */

@theme {
    /* design tokens — colors, fonts, breakpoints overrides if needed */
}
```

The `tailwindcss` standalone CLI v4 (single binary, ~50 MiB) reads
this, scans the listed `@source` paths for class usage, and writes the
compiled CSS. The `build.rs` invokes the CLI:

```
tailwindcss -i web/style.css -o $OUT_DIR/style.css --minify
```

`include_bytes!(concat!(env!("OUT_DIR"), "/style.css"))` embeds the
result. The build script locates the CLI similarly to how
`prost-build` locates `protoc` — env var override
(`COCO_TAILWIND_CLI`), `$PATH` lookup, then a vendored fallback in
`hub/server/vendor/tailwindcss-<platform>`.

#### 6.2.1 Prism language bundles to ship in V1

Languages bundled by default — chosen for what an agent actually emits
or operates on:

| Language | Why |
|----------|-----|
| `json` | Tool inputs / outputs / payload JSON view (highest-volume use) |
| `bash` | Shell tool commands |
| `python` | Common project language |
| `javascript` / `typescript` | Common project language |
| `rust` | Self-host project language |
| `go` | Common project language |
| `yaml` | Configs, frontmatter |
| `markdown` | Skill / command source bodies |
| `sql` | DB tool outputs |
| `diff` | `apply-patch` tool input |

Total bundled JS size ~120 KiB minified-gzipped, acceptable for a
single-page operator tool. No autoloader (would require external CDN
and break the single-binary promise); a code block with an unbundled
language falls back to plain `<pre>` rendering.

The Askama base template loads Prism in the page head:

```html
<link rel="stylesheet" href="/static/prism-tomorrow.min.css">
<script defer src="/static/prism.min.js"></script>
<script defer src="/static/prism-json.min.js"></script>
{# … other language bundles … #}
```

Prism auto-highlights `<pre><code class="language-json">…</code></pre>`
on page load and after HTMX swaps (we wire a `htmx:afterSwap` listener
to call `Prism.highlightAllUnder(target)`).

### 6.3 Page routes (server-rendered HTML)

| Method | Path | Purpose | Template |
|--------|------|---------|----------|
| `GET` | `/` | Dashboard: recent activity + summary KPIs | `dashboard.html` |
| `GET` | `/i` | All instances (paged table) | `instances.html` |
| `GET` | `/i/:instance_id` | Instance detail: attributes + sessions list | `instance.html` |
| `GET` | `/i/:instance_id/s/:session_id` | Session timeline (events) | `session.html` |
| `GET` | `/i/:instance_id/s/:session_id/agents` | Sub-agent tree view | `session_agents.html` |
| `GET` | `/search` | Search results page | `search.html` |
| `GET` | `/about` | Hub version + protocol versions + counts | `about.html` |

### 6.4 HTMX partial routes (HTML fragments)

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/p/instances` | Instance table body — paged with cursor |
| `GET` | `/p/sessions?instance=…` | Sessions table body for an instance |
| `GET` | `/p/events?session=…&before=…&kind=…` | Event timeline slice |
| `GET` | `/p/event/:rowid/expand` | Single event's full JSON payload (modal) |
| `GET` | `/p/search?…` | Search results body |
| `GET` | `/p/agent-tree?session=…` | Sub-agent SVG/HTML tree |

Conventions:

- Path prefix `/p/` for "partials"; never reached directly except via
  HTMX swap.
- All partial responses include the `Vary: HX-Request` header so a
  caching layer can distinguish full page vs fragment.
- Partials accept a `hx-target` from the caller's HTMX attributes;
  the handler is naive about *where* it gets swapped in.

### 6.5 SSE — live session updates

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/sse/session/:instance_id/:session_id` | `text/event-stream`; pushes an HTML fragment per new event |

Internally:

```
WS reader  ──┐
             │  ingest_batch()
             ├──► storage writer (SQLite)
             │
             └──► broadcast::Sender<EventEnvelope>  (per-session topic)
                                    │
                                    └──► SSE subscribers (browsers
                                          watching this session)
```

Each broadcast subscriber renders one HTML `<event>` fragment from a
shared Askama partial and writes one SSE `data: …` line. HTMX on the
browser side uses `hx-ext="sse"` and swaps the fragment into the
event-list container.

Lifetime / disconnect: SSE connections die when browser closes tab;
the broadcast subscriber drops naturally. No reconnect bookkeeping
needed on the server.

### 6.6 Component aesthetic

- Default theme: Flowbite default (light + dark mode toggle); Prism's
  `prism-tomorrow.min.css` chosen because it composes cleanly with
  Flowbite dark mode.
- Layout: top nav (instances · sessions · search · about), sticky
  filter sidebar on list pages, central content.
- Tables: Flowbite striped tables with row-click → `hx-get` for detail.
- Modals: Flowbite modal component for expanded event payload — the
  raw envelope JSON renders inside
  `<pre><code class="language-json">…</code></pre>` and Prism
  highlights on swap.
- Tool outputs in event detail use the language hint when one is
  obvious (Shell → `bash`, apply-patch → `diff`, file read with `.json`
  extension → `json`); otherwise plain `<pre>` with no class — Prism
  leaves it alone.

### 6.7 Accessibility & responsive

- Flowbite components are WCAG-AA where their docs claim so; we don't
  add a separate audit in V1.
- Mobile: hub is an operator tool, not a phone app; mobile layout
  degrades to "readable but not optimized".

---

## 7. JSON query API (for external consumers and round-5 SDKs)

Parallel to the HTML routes, the hub exposes a JSON API. The Web UI
does **not** consume this directly (it consumes the HTML partials);
the JSON API exists for IDEs, scripts, dashboards, etc.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/v1/instances` | Paged JSON list of instances |
| `GET` | `/v1/instances/:instance_id` | Instance row + attributes |
| `GET` | `/v1/instances/:instance_id/sessions` | Paged sessions |
| `GET` | `/v1/instances/:instance_id/sessions/:session_id/events` | Paged events |
| `GET` | `/v1/instances/:instance_id/sessions/:session_id/agents` | Sub-agent edges |
| `GET` | `/v1/search` | Cross-cutting search |
| `GET` | `/v1/protocol` | Supported protocol versions |
| `GET` | `/healthz` | Liveness |

### 7.1 `GET /v1/search` query parameters (V1)

V1 search is **structured-filter only** — no free-text input. The `q`
parameter is reserved at the trait level (§2.4) but the V1 HTTP
surface returns `HTTP 400 Bad Request` if it's set:

```json
{ "error": "free_text_not_supported",
  "detail": "Full-text search is deferred to V2. Use structured filters." }
```

| Param | Type | Meaning |
|-------|------|---------|
| `instance` | string | Filter to one instance |
| `session` | string | Filter to one session (requires `instance`) |
| `agent` | string | Filter by `agent_id` |
| `kind` | string | Filter by outer `kind` (`protocol`, `tool_use_completed`, …) |
| `inner_kind` | string | Filter by Protocol inner kind (`turn_started`, etc.) |
| `tool` | string | Filter by `tool_name` |
| `error` | `0`/`1` | Filter by `is_error` |
| `from` | RFC 3339 | `ts >= from` |
| `to` | RFC 3339 | `ts <  to` |
| `limit` | int | 1..500, default 50 |
| `cursor` | opaque | Pagination cursor |

Response:

```json
{
  "results": [
    {
      "instance_id": "…",
      "session_id": "…",
      "seq": 1234,
      "ts": "2026-05-16T12:34:56.789Z",
      "kind": "tool_use_completed",
      "inner_kind": null,
      "tool_name": "Shell",
      "agent_id": "main",
      "is_error": false,
      "preview": "…snippet from the events.preview column…"
    }
  ],
  "next_cursor": "eyJzZXEiOjEyMzQsInRzIjoxNzc4OTI2MzQ5MzE1fQ==",
  "took_ms": 12
}
```

The cursor is base64-encoded JSON `{seq, ts}`; the next request resolves
to `WHERE (ts, seq) < (cursor.ts, cursor.seq)` for stable ordering.

When FTS lands in V2, `q` simply starts being honored (and the Web UI's
search page gains a free-text input). The response shape doesn't change
— `preview` will then include a *match-relevant* snippet rather than
a leading slice.

---

## 8. Retention & vacuum

### 8.1 Defaults

> User decision, round 4: keep retention tight by default — this is a
> per-operator analysis surface, not a long-term archive.

| Knob | Default | Purpose |
|------|---------|---------|
| `hub_retention_days` | `3` | Delete events older than N days |
| `hub_retention_max_bytes` | `3221225472` (3 GiB) | Total DB-file ceiling |
| `hub_retention_sweep_interval_secs` | `900` (15 min) | Background sweep cadence — tighter to match tighter caps |
| `hub_vacuum_threshold_bytes` | `268435456` (256 MiB) freed | Trigger `VACUUM` after this much deletion |

Operators who need longer history override via `settings.json` /
`COCO_*` env / CLI. The schema and sweep algorithm scale with any
value; the defaults are tuned for "I just want to see what my agents
have been doing recently."

### 8.2 Sweep algorithm

A background tokio task runs every `hub_retention_sweep_interval_secs`:

1. `DELETE FROM events WHERE received_at < now - retention_days * 86400_000`.
2. If DB size still > `retention_max_bytes`, delete the oldest
   *complete sessions* (by `last_event_ts`) until under cap.
   "Complete session" = `ended_at IS NOT NULL` OR last event > 24 h ago.
3. After every sweep, run `PRAGMA incremental_vacuum;` to release pages.
4. If freed pages cumulatively exceed `hub_vacuum_threshold_bytes`,
   run `VACUUM;` (full rewrite) opportunistically when no writes have
   happened in the last 60 s.

FTS5 entries are deleted via cascading manual DELETE (FTS5 has no
foreign-key triggers); the sweep deletes from `events_fts` first, then
`events`, in one transaction per pruned session.

### 8.3 Per-instance overrides

Reserved for round 5+. V1 has global retention only — keeps the
schema and admin UI minimal.

---

## 9. Hub-side configuration

In addition to the connector-side knobs from §8 of round 3, the hub
binary reads its own settings (the hub is a separate process, except
when embedded via `--serve-hub`):

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `hub_data_dir` | path | `~/.coco/hub/` (embedded) or `$PWD/data/` (standalone) | SQLite + assets dir |
| `hub_bind_addr` | string | `127.0.0.1` | Bind address |
| `hub_port` | u16 | `8731` | Port |
| `hub_max_frame_bytes` | u32 | `10485760` (10 MiB) | Reject frames above this |
| `hub_max_tool_output_inline` | u32 | `262144` (256 KiB) | Truncate beyond |
| `hub_retention_days` | u32 | `3` | See §8 |
| `hub_retention_max_bytes` | u64 | `3221225472` (3 GiB) | See §8 |
| `hub_retention_sweep_interval_secs` | u32 | `900` (15 min) | See §8 |
| `hub_vacuum_threshold_bytes` | u64 | `268435456` (256 MiB) | See §8 |
| `hub_log_level` | string | `info` | Standard tracing filter |
| ~~`hub_auth_token`~~ | — | — | **Removed from V1.** Auth is not implemented this round — see §8.4 / §11. |

CLI for standalone mode (`coco-hub-server` binary, or `coco --serve-hub`):

```
coco-hub-server serve
  --bind 127.0.0.1
  --port 8731
  --data-dir ./data
```

Embedded mode (`coco --serve-hub`) reuses the agent's `~/.coco/hub/`
data dir by default. **No `--token` flag in V1** — auth is not
implemented.

---

## 10. Crate layout — three crates under `coco-rs/hub/`

> Round-4 user decisions:
> 1. *"coco-rs/hub 新增 1 个子目录，在下面放 hub 相关的 crate"*.
> 2. *"coco-rs/hub 下面要分 3 个 crate：coco-hub-protocol, coco-hub-connector, coco-hub-server"*.
> 3. *"避免 coco-rs cli 引入 coco-hub-server 的依赖"*.

Exactly **three** crates live under `coco-rs/hub/`. The store
abstraction is a module inside `coco-hub-server`, not a separate
crate. `coco-rs/app/cli` depends on **protocol + connector
unconditionally** and on **server via an opt-in Cargo feature**
(round-4 follow-up decision: *"默认版本不带 hub-server，如果要指定，
走 feature"*). Default `coco` builds stay lean; users who want the
embedded hub server pass `--features serve-hub`.

| Path | Crate name | Role | Runs | In default `coco` build? |
|------|------------|------|------|--------------------------|
| `coco-rs/hub/protocol/`  | `coco-hub-protocol`  | Wire types only (frames, envelope, payload, close codes) | both sides | yes (always) |
| `coco-rs/hub/connector/` | `coco-hub-connector` | Agent-side: aggregator + ring buffer + WS client | agent | yes (always) |
| `coco-rs/hub/server/`    | `coco-hub-server`    | Hub-side: `EventStore` trait + SQLite impl + Axum router + Web UI + SSE + retention | hub | **no by default** (optional dep behind Cargo feature `serve-hub`); opt in with `--features serve-hub` |

### 10.1 Why three, not four

In an earlier round-4 draft the store was a fourth crate (`coco-hub-store`).
The user decision compressed it because:

- Nothing outside `coco-hub-server` ever needs the store trait. A
  future Postgres impl will be a `feature = "postgres"` module inside
  `coco-hub-server`, not a separate crate that downstream consumers
  link against.
- Splitting the store would add one more `Cargo.toml` + workspace
  member without buying any consumer benefit.
- The trait abstraction (§2.4) is preserved as the internal
  `coco_hub_server::store::EventStore` boundary — call sites in the
  rest of `coco-hub-server` already program against `Arc<dyn EventStore>`,
  so swap-the-backend is unchanged.

### 10.2 Module map per crate

```
coco-rs/hub/
├── protocol/                              (coco-hub-protocol)
│   └── src/
│       ├── lib.rs               ── pub re-exports
│       ├── frame.rs             ── AnnounceFrame, BatchFrame, AckFrame, ErrorFrame, …
│       ├── envelope.rs          ── EventEnvelope, EventPayload (tagged union)
│       ├── close_code.rs        ── WS close codes (4000–4029)
│       └── version.rs           ── Sec-WebSocket-Protocol constants
│
├── connector/                             (coco-hub-connector)
│   └── src/
│       ├── lib.rs               ── Connector::spawn(config) -> ConnectorHandle
│       ├── aggregator.rs        ── §6 state machine of round 3
│       ├── ring.rs              ── bounded in-memory ring buffer
│       ├── batcher.rs           ── §7.1 batching of round 3
│       ├── ws_client.rs         ── tokio-tungstenite client + reconnect loop
│       └── reader.rs            ── ack/error frame consumer
│
└── server/                                (coco-hub-server)
    ├── Cargo.toml               ── features:
    │                                  default      = ["sqlite"]
    │                                  sqlite       = [rusqlite, deadpool-sqlite]
    │                                  postgres     = [tokio-postgres, …] (future)
    │                                  test-util    = []  (exposes MockEventStore)
    ├── build.rs                 ── invoke tailwindcss to build style.css → OUT_DIR
    ├── web/                                       (sibling, not Rust)
    │   ├── templates/*.html              ── Askama templates (mobile-first)
    │   ├── style.css                     ── Tailwind input
    │   └── static/
    │       ├── htmx.min.js, htmx-ext-sse.min.js
    │       ├── flowbite.min.js
    │       ├── prism.min.js, prism-<lang>.min.js (11 langs), prism-tomorrow.min.css
    │       └── favicon, fonts
    └── src/
        ├── lib.rs                ── Axum router assembly; takes Arc<dyn EventStore>
        ├── main.rs               ── `coco-hub-server` standalone binary
        ├── store/                ── §2.4 trait + impls
        │   ├── mod.rs                ── pub trait EventStore + re-exports
        │   ├── model.rs              ── InstanceRow, SessionRow, EventRow, AgentEdge, SearchHit
        │   ├── query.rs              ── EventQuery, SearchQuery, EventFilter, Page, Cursor
        │   ├── error.rs              ── EventStoreError
        │   ├── retention.rs          ── RetentionPolicy, SweepStats
        │   ├── mock.rs               ── feature = "test-util" — MockEventStore
        │   └── sqlite/               ── feature = "sqlite" (default)
        │       ├── mod.rs                  ── SqliteEventStore
        │       ├── schema.rs               ── DDL + migrations (§3)
        │       ├── ingest.rs               ── §4
        │       ├── search.rs               ── structured-filter compiler (§7.1)
        │       ├── retention.rs            ── sweep + vacuum (§8)
        │       └── cursor.rs               ── (seq, ts) ↔ base64 round-trip
        ├── ingest_pipeline.rs    ── wraps EventStore::ingest_batch + fans out to LiveTopics
        ├── live_topics.rs        ── per-session broadcast::Sender<EventEnvelope> registry
        ├── ws/mod.rs             ── /v1/connect WS accept (Axum WebSocket)
        ├── web/mod.rs            ── Askama page handlers
        ├── web/partials.rs       ── /p/* HTMX fragment handlers
        ├── web/sse.rs            ── /sse/session/<i>/<s>
        ├── web/static_assets.rs  ── include_dir! of web/static + OUT_DIR/style.css
        ├── api/mod.rs            ── /v1/* JSON handlers
        └── retention.rs          ── background task calling EventStore::run_retention_sweep
```

### 10.3 Default-off with explicit opt-in — the `serve-hub` feature

Default `coco` builds **exclude** `coco-hub-server`. Operators who
want the embedded hub server pass `--features serve-hub` (or use
the `just coco-with-hub` recipe).

```toml
# coco-rs/app/cli/Cargo.toml
[features]
default      = []                              # default = no hub server
serve-hub    = ["dep:coco-hub-server"]

[dependencies]
coco-hub-protocol  = { path = "../../hub/protocol" }
coco-hub-connector = { path = "../../hub/connector" }
coco-hub-server    = { path = "../../hub/server", optional = true }
```

Build matrix:

| Command | Hub server compiled in? | `--serve-hub` flag works? |
|---------|-------------------------|---------------------------|
| `cargo build -p coco-cli` | no | flag errors at runtime |
| `cargo build -p coco-cli --features serve-hub` | **yes** | yes |
| `cargo build -p coco-hub-server` (standalone) | n/a (it *is* the hub) | n/a |

The CLI flag `--serve-hub` is **always present** in `--help`. When
the `serve-hub` feature was not compiled in, invoking it fails with
a clear message:

> *"This `coco` build was not compiled with the `serve-hub` feature.
> Either rebuild with `--features serve-hub`, or run a separate
> `coco-hub-server` process and point `event_hub_url` at it."*

#### 10.3.1 Why default-off

By keeping `serve-hub` opt-in we get:

| Benefit | Why it matters |
|---------|----------------|
| Default `cargo build` stays fast | No axum / sqlite / askama / Tailwind asset build for everyone who just wants `coco` the agent. |
| Default `coco` binary stays small | ~10–20 MiB less than the hub-included build (no embedded htmx/flowbite/prism + 11 lang bundles + Tailwind CSS + statically linked SQLite). |
| `tailwindcss` CLI not required for default builds | Only contributors touching the hub server need to install / vendor it. |
| A hub-crate compile error never breaks the default `coco` build | Iteration on the hub doesn't block agent work. |
| Distribution is still trivial | We ship two pre-built binaries (`coco` + `coco-hub-server`) or one combined build (with `--features serve-hub`). Distros pick. |

The cost is one extra step to get the embedded all-in-one mode
(`--features serve-hub`), which we hide behind a `just` recipe.

#### 10.3.2 `just` recipes wrapping per-package Cargo features

Cargo features are per-package; from the workspace root we still
need `-p`. `justfile` hides this:

```just
# Build the default coco binary (NO embedded hub server).
coco:
    cargo build -p coco-cli

# Build coco with the embedded hub server (all-in-one mode).
coco-with-hub:
    cargo build -p coco-cli --features serve-hub

# Build the standalone hub-server binary.
hub-server:
    cargo build -p coco-hub-server
```

Operators use `just coco` / `just coco-with-hub` / `just hub-server`
and never need to know the underlying Cargo invocation. The existing
`just quick-check` / `just pre-commit` recipes from CLAUDE.md remain
the iteration gates; `coco-*` recipes are explicit-output builds.

### 10.4 External crates that change

- **CHANGED** `coco-rs/app/cli/`
  - `Cargo.toml`: depends on `coco-hub-protocol` + `coco-hub-connector`
    unconditionally; **`coco-hub-server` is an optional dep behind the
    `serve-hub` Cargo feature, which is `default = []`**. Plain
    `cargo build` does not include the hub server; `--features
    serve-hub` opts in.
  - `src/`: wires `--event-hub-url` / `--serve-hub` / `--hub-port`
    flags into bootstrap. `#[cfg(feature = "serve-hub")]` gates the
    server-spawn path; when the feature is off the flag prints the
    error message in §10.3.
- **CHANGED** `coco-rs/app/query/src/engine.rs` (or wherever the
  `CoreEvent` sender is owned) — clone an additional
  `Sender<CoreEvent>` for the connector when `event_hub_url` is set;
  spawn `coco-hub-connector` and route to it.
- **CHANGED** `coco-rs/app/session/src/concurrent_sessions.rs` — add
  `instance_id: Uuid` to `SessionRegistration` (already noted in
  round 3).
- **CHANGED** `coco-rs/common/config/src/` — register six
  `event_hub_*` keys (for the connector) + eleven `hub_*` keys (for
  the server) in `EnvKey` and the layered settings stack. The
  server-side `hub_*` keys are present in the schema regardless of
  whether the `serve-hub` feature is compiled — they're inert when
  unused.

### 10.5 Dependency graph

```
coco-hub-protocol ──┐
                    ├──► coco-hub-connector  (agent side)
                    │
                    └──► coco-hub-server     (hub side; optional dep of cli)
                              │
                              └── (internal) store::EventStore trait ─┬─ SqliteEventStore
                                                                      │   (feature = "sqlite", default)
                                                                      ├── MockEventStore
                                                                      │   (feature = "test-util")
                                                                      └── (future) PgEventStore
                                                                          (feature = "postgres")
```

The dependency cli → server goes through one Cargo feature flip; the
trait abstraction stays internal to the server crate; no external
consumer needs to link against the store impls.

---

## 11. What is intentionally **not** specified here

Per the round-3 §10 list, still:

- **Auth model.** Not implemented in V1 (user decision: *"先忽略鉴权"*).
  The hub binds wherever it's told to bind and accepts every
  connection. Bearer-token / mTLS / OIDC will get a dedicated round
  after V1 ships.
- **Phase-3 control plane** (parked) — WebSocket frame kinds for
  cancel/approve/inject/permission-mode.
- **OTLP export** (post-V1) — translating events to OTLP spans.
- **Multi-hub federation** (post-V1).
- **Per-instance retention overrides** (post-V1).
- **Blob storage for huge tool outputs** (post-V1).
- **Detailed per-template HTML structure** — that's implementation,
  not contract.
- **FTS5 / free-text search** — deferred to V2 (this round's
  decision; the trait surface is ready).

---

## 12. Open items for round 5

### 12.1 Settled this round (between round-4 follow-up turns)

- ✅ **`just coco-with-hub` is NOT part of `just pre-commit`.** Pre-commit
  only builds + tests the default `just coco`. The `with-hub` variant
  is verified through a different path (§12.2 item A).
- ✅ **Phase-1 DoD gates only on `just coco` passing.** The default
  build is the hard requirement; the `with-hub` build's status is
  reported but not blocking.

### 12.2 Resolved this round (round-4 follow-up turns)

| # | Item | Decision |
|---|------|----------|
| A | `coco-with-hub` verification | **No CI gate.** `just coco-with-hub` is provided as a manual-invocation recipe; users / hub maintainers run it when they care. Early phase explicitly accepts the rot risk per user: *"前期不关注质量问题"*. Adding a paths-filter CI job is a free post-V1 follow-up. |
| B | Tailwind v4 CLI sourcing | **Contributors install `tailwindcss` themselves.** No vendoring, no network download. `build.rs` searches `COCO_TAILWIND_CLI` → `$PATH`; if missing, prints `"tailwindcss v4 CLI not found. Install from https://tailwindcss.com/blog/standalone-cli or set COCO_TAILWIND_CLI."` and fails the build. Only `--features serve-hub` builds touch this. |
| C | Hub UI dev-loop | `cargo watch -x 'run -p coco-hub-server'` in one terminal + `tailwindcss --watch` in another. Hub-server gains a `--dev-assets <dir>` flag that reads CSS/JS from disk instead of the embedded copy when set, so the watcher's output is hot. Documented in `hub/server/CLAUDE.md`. |
| D | Workspace `default-members` | `coco-rs/Cargo.toml`'s `default-members = ["app/cli"]`. Plain `cargo build` at the workspace root builds only `coco`; use `cargo build --workspace` for everything. |
| E | Hub-feature smoke test | Per the suggested list (single-machine all-in-one → multi-instance → resilience → `/clear` rotation). Codified in round-5 §6. |
| F | Static asset version pins | Tailwind v4.0.x, Flowbite v2.x, HTMX v1.9.x, Prism v1.29.x. JS/CSS committed under `coco-rs/hub/server/web/static/`. Updates via manual bump PR. |

### 12.3 Implementation details to address during coding (no round-5 decision needed)

These are mechanical from CLAUDE.md + existing patterns. Listed so we
don't forget them.

- **Layer placement** in CLAUDE.md's L0–L5 hierarchy:
  - `coco-hub-protocol` → **L1** (depends only on `coco-types` re-exports + `serde`, no other internal deps).
  - `coco-hub-connector` → **L3** (depends on protocol + coco-types + utils + common/otel).
  - `coco-hub-server` → **L5** (depends on protocol + heavy deps; sits alongside `coco-cli` / `coco-tui`).
- **`EnvKey` registration** for the 6 `event_hub_*` env vars (connector side) and 11 `hub_*` env vars (server side) in `coco-config::EnvKey`.
- **`StatusCode` allocations** in `coco-error`: reserve category `EventHub = 14` with sub-codes (connector send fail, hub protocol mismatch, etc.); document in `common/error/README.md`.
- **Workspace member declaration** in `coco-rs/Cargo.toml`: add the three `hub/*` paths to `members`.
- **Per-crate `CLAUDE.md`** for each new crate (project convention).

### 12.4 Items removed from the round-5 list

- ~~Auth model~~ — user deferred to a later dedicated round.
- ~~Crate layout finalization~~ — settled in round 4 §10.
- ~~`--features serve-hub` invocation~~ — settled in §10.3.2;
  `default = []`, opt-in via `--features serve-hub`, wrapped by
  `just coco-with-hub`.
- ~~`pre-commit` integration of `with-hub` variant~~ — settled this
  round: not in pre-commit.
- ~~DoD includes `with-hub` build~~ — settled this round: only `just
  coco` is a hard requirement.
