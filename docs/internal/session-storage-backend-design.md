# Session Storage Backend — pluggable persistence design (v2)

> Status: **design / for review** (no code yet). v2 incorporates the
> adversarial review — see [§12 Changelog](#12-changelog-v1--v2).
> Scope: `app/session` (`coco-session`) + the tool-result blob path in
> `core/tool-runtime` + their ~12 injection sites.
> Goal: decouple session persistence from local disk so the same domain
> logic runs against a remote backend (DB / HTTP / object store) for
> remote state recovery — **phased**, with the on-disk JSONL store as the
> default and a write-mirror decorator as the cheap intermediate step.

Coco-rs-native design (no TS counterpart). Follows the existing
store-trait idiom ([`PermissionStore`](crate-coco-permissions.md),
`ScheduleStore`): *the crate never touches the filesystem directly — it
only transforms/derives; the store trait is the boundary.*

## 0. Decisions locked & this-iteration scope

Resolved from the v2 review:

- **Backend selection** → a resolved `RuntimeConfig` field `session.backend`
  (enum; `Disk` is the only variant this iteration). No new `COCO_*` env,
  per the project's "consume `RuntimeConfig`, never raw env" rule.
- **Tool-result blobs** → **local-only disk cache.** The truncated
  `<persisted-output>` preview already rides in the transcript `Entry`
  stream, so it travels to any backend for free; the full body stays local
  and full-fidelity re-fetch **degrades** on a host without it (accepted).
  ⇒ **no `SessionBlobStore` trait this iteration** — §3.4 is deferred.
- **Scope** → implement the **trait boundary + the `Disk*` backend only.**
  `TeeStore`, `RemoteStore`, the `SessionSummary`/`StorageStat` split (§3.2),
  the `recovery`→`&[Entry]` derive extraction (§3.1), and golden-equivalence
  tests (§6.1) are **deferred until a second (non-fs) backend lands** — under
  disk-only they add churn without payoff (the disk impl *is* today's code,
  so there is no equivalence risk to gate).

**Shipped:** `app/session/src/store.rs` — the traits
`TranscriptIo` / `AgentTranscriptStore` / `UsageSnapshotStore` / `SessionStore`
(blanket-composed) / `SessionCatalog`, plus `ResolvedSession`; the `Disk*`
impls (`TranscriptStore` implements the IO traits; `DiskCatalog` the catalog),
each delegating to today's `std::fs` logic verbatim; `SessionError::Backend`;
and `SessionManager` holds an `Arc<dyn SessionCatalog>` routing
`load`/`list`/`resolve`/`delete` through the boundary.

A second pass then landed:

- **`InMemoryStore` / `InMemoryCatalog`** — a pure-RAM backend that satisfies
  the same traits, reusing the disk store's exact chain-dedup
  (`storage::build_message_chain_entries`) and metadata-derivation
  (`storage::fold_transcript_metadata`, `marble_origami_entries`) helpers,
  extracted as pure free functions so both backends share one code path.
- **`SessionBackend` selector** — resolved `RuntimeConfig` field
  `session.backend` (`disk` default / `memory`); `store::catalog_for_backend`
  is the single match site. The two construction points are config-driven:
  `SessionManager::with_backend` and the per-turn engine store in
  `session_runtime` (which sources its `Memory` store from the manager's
  catalog so engine + manager share one ephemeral state).
- **Catalog read path completed** — `SessionCatalog::read_metadata` / `delete`
  removed the last consumers that reached into `ResolvedSession.transcript_path`
  + the free `read_transcript_metadata_at`, so `SessionManager` is fully
  backend-agnostic.
- A **swap test** (`store.test.rs`) runs the same workload through `dyn`
  trait objects against both backends and asserts identical content-derived
  metadata + chain shape — the "the traits aren't carved to disk's shape"
  proof.

Scope note: `TranscriptFileHistorySink` stays a concrete disk
`TranscriptStore` on purpose (local-only checkpoint data, not authoritative —
§3.4); even under `Memory` it writes to disk.

**Still deferred** (land with the first non-fs *authoritative* backend, where
back-compat is not a constraint): `TeeStore`, `RemoteStore`, the
`SessionSummary`/`StorageStat` metadata split (§3.2), the `recovery` →
`&[Entry]` derive extraction (§3.1), and golden-equivalence tests (§6.1).

## 1. Goals / non-goals

**Goals**

- A backend boundary that abstracts **every authoritative disk operation**
  session state depends on — transcript JSONL **and** the tool-result
  blob sidecars — with disk impls as the default and **zero behavior
  change** under disk.
- Keep the door open for an authoritative **remote** backend so a fresh /
  diskless host can `resume` from remote state.
- A **write-mirror** path (backup / Event Hub) that is *the same boundary*
  (a decorator), promotable to authoritative — not a parallel code path.
- Minimal blast radius: call sites change `Arc<Struct>` → `Arc<dyn Trait>`;
  method calls are unchanged.

**Non-goals (this iteration)**

- Choosing a concrete remote backend (SQL/HTTP/object store) — the traits
  are backend-agnostic; `RemoteStore` is deferred ([§10](#10-open-questions)).
- Remote-izing the **PID registry** (`SessionRegistry`, keyed by OS PID +
  `libc::kill`) — host-local; fleet liveness is the Event Hub's job ([§9](#9-out-of-scope-keep-local)).
- Remote-izing **`PromptHistory`** — UX-local input recall; deferred ([§9](#9-out-of-scope-keep-local)).

## 2. Current coupling (audit)

`coco-session` is **100% synchronous** (`std::fs`, no tokio). Disk-coupled
surface:

| Module | Disk dependency | Nature |
|---|---|---|
| `storage::TranscriptStore` | `std::fs` everywhere — append-only JSONL, bulk `load_entries` (refuses >50 MB), head/tail-windowed `read_metadata`, `list_sessions`, usage/agent sidecars, tool-result *cleanup* | **Core seam** |
| `storage` free fns | `resolve_session_file_path` (git-worktree shell-out + dir scan), `list_all_sessions` | Cross-project resolution; fs/slug-specific |
| `recovery::*` | `&Path` + `std::fs::File`, `parent_uuid` DAG walk, bespoke parse (pulls `slug` off raw `Value`, skips sidechain) | **Pure logic** once entries are in memory |
| `lib::SessionManager` | Spans `<memory_base>/projects/*/`; title/tag/mode/cleanup | Domain layer |
| **`core/tool-runtime::tool_result_storage`** | **Writes full tool-result bodies to `<session_dir>/tool-results/<id>.{txt,json}` via raw paths — never through `TranscriptStore`** | **Second seam (see §3.4)** |
| `history::PromptHistory` | `history.jsonl` + paste-store, `fs2` lock | Separate concern (config_home) |
| `concurrent_sessions::SessionRegistry` | PID files + `libc::kill` | Host-local |

**Key insight:** the domain logic — resume DAG walk (`recovery.rs`),
metadata derivation, chain dedup, file-history / marble-origami replay —
operates on in-memory `Vec<Entry>`. Only **IO**, **resolution** (slugs,
worktree shell-out, head/tail windowing), and **fs-stat** are
filesystem-shaped. The seam lets that logic stay in one place and run
unchanged against any backend.

### 2.1 Consumer / hot-path facts

- **Hot write path**: `append_message_chain` runs **once per LLM turn** at
  `app/query/src/engine_finalize_turn.rs:1030`, synchronously inside an
  async fn (not awaited, not `spawn_blocking`).
- **Slow ops mostly protected**: usage snapshots use `spawn_blocking`
  (`session_runtime.rs:1688`, `engine_builder.rs:294`). **But** the SDK
  resume path (`handlers/session.rs:849-872`) calls `resolve` +
  `load_conversation_for_resume` **directly on the async executor** — a
  remote backend must not block there (see §3.6).
- **Prior art**: `app/cli/.../agent_transcript_persistence.rs` already has
  an *async* `AgentTranscriptStore` trait bridging the sync store via
  `spawn_blocking`. v2 subsumes it (§3.3).

## 3. Design

### 3.1 Seam at `Entry` granularity, pure derivation shared

Traits operate on typed `Entry`, **not** opaque byte lines (a DB wants
rows; consumers already think in `Entry`). Serialization (Entry↔JSONL vs
Entry↔columns) is each backend's private business.

Derivation stays as **pure free functions** every backend reuses:

```rust
// storage/derive.rs — pure, backend-independent, no IO
pub fn summary(entries: &[Entry], session_id: &str) -> SessionSummary;
pub fn session_resume_state(entries: &[Entry]) -> SessionResumeState; // ex-recovery.rs
pub fn build_message_chain(msgs, seen, opts) -> (Vec<Entry>, ChainWriteResult);
```

`recovery.rs` is refactored to consume `&[Entry]` (resume =
`store.load_entries(id)` → `derive::session_resume_state(&entries)`),
removing fs coupling from the heaviest logic and making it unit-testable
without a tempdir. Note (M4): the current `recovery.rs:97-132` parse loop
diverges from `parse_entry` (reads `slug` off the raw `Value` → now must
read it from `TranscriptEntry.extra`; skips sidechain inline). Unifying
them is a real migration — gated by golden tests (§6.1), not assumed free.

### 3.2 Split metadata: content-derived vs storage-stat *(fixes C3)*

The current `TranscriptMetadata` conflates content-derived fields with
**fs-stat** fields (`created_at`/`modified_at`/`file_size` come from
`std::fs::metadata`, not entries). A pure deriver cannot produce them, and
deriving over `load_entries` would also hit the 50 MB refusal. Split:

```rust
/// Derivable purely from entries — same fn for every backend.
pub struct SessionSummary {
    pub session_id: String,
    pub first_prompt: String,
    pub message_count: i32,
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    pub tag: Option<String>,
    pub last_prompt: Option<String>,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub is_sidechain: bool,
    // … agent_name/color/setting/mode/worktree_state/pr_link …
}

/// Backend-supplied storage facts. Not all backends have a byte size.
pub struct StorageStat {
    pub created_at_ms: Option<u128>,
    pub modified_at_ms: u128,   // disk: mtime; DB: max(entry ts) / row mtime
    pub size_bytes: Option<u64>,
}
```

`read_metadata` becomes two methods so the default impl is *honest*:

```rust
fn summary(&self, sid: &str) -> Result<SessionSummary> {
    Ok(derive::summary(&self.load_entries(sid)?, sid)) // default; disk overrides w/ head/tail window
}
fn stat(&self, sid: &str) -> Result<Option<StorageStat>>; // no default — backend-specific
```

The disk backend overrides `summary` with its head/tail-window fast path
(and should **stream** rather than inherit the 50 MB hard refusal —
back-compat is not a constraint).

### 3.3 Split traits (ISP) *(fixes M2)*

One fat 14-method trait forced every backend to stub usage snapshots and
collided with the existing async agent trait. Split along natural seams;
compose where a backend wants one object:

```rust
/// Per-project transcript IO — the hot path.
pub trait TranscriptIo: Send + Sync {
    fn append_entries(&self, sid: &str, entries: &[Entry]) -> Result<()>; // durable-on-return (§3.5)
    fn load_entries(&self, sid: &str) -> Result<Vec<Entry>>;
    fn exists(&self, sid: &str) -> bool;
    fn delete(&self, sid: &str) -> Result<()>;
    fn stat(&self, sid: &str) -> Result<Option<StorageStat>>;
    fn summary(&self, sid: &str) -> Result<SessionSummary> { /* default over load_entries */ }
    fn list(&self) -> Result<Vec<(SessionSummary, StorageStat)>>; // this project
}

/// Subagent transcripts — typed on Message, not Entry. Subsumes the
/// existing cli AgentTranscriptStore (which becomes a thin adapter).
pub trait AgentTranscriptStore: Send + Sync {
    fn append_agent_messages(&self, sid: &str, agent_id: &str, msgs: &[Arc<Message>]) -> Result<()>;
    fn load_agent_messages(&self, sid: &str, agent_id: &str) -> Result<Option<Vec<Arc<Message>>>>;
    fn write_agent_metadata(&self, sid: &str, agent_id: &str, m: &AgentMetadata) -> Result<()>;
    fn read_agent_metadata(&self, sid: &str, agent_id: &str) -> Result<Option<AgentMetadata>>;
}

/// Cumulative per-session usage snapshot.
pub trait UsageSnapshotStore: Send + Sync {
    fn write_usage_snapshot(&self, sid: &str, snap: &SessionUsageSnapshot) -> Result<()>;
    fn load_usage_snapshot(&self, sid: &str) -> Result<Option<SessionUsageSnapshot>>;
}

/// Cross-project catalog + backend factory. SessionManager delegates here.
pub trait SessionCatalog: Send + Sync {
    fn store_for(&self, cwd: &Path) -> Arc<dyn TranscriptIo>; // cheap / pooled (m1)
    fn resolve(&self, sid: &str, cwd_hint: Option<&Path>) -> Result<Option<ResolvedSession>>;
    fn list_all(&self) -> Result<Vec<(SessionSummary, StorageStat)>>;
    fn cleanup_older_than(&self, older: Duration) -> Result<i32>;
}
```

All methods are object-safe (no generics — `append_message_chain`'s
`IntoIterator` lives in `derive::build_message_chain`). Helpers
(`append_message`/`append_metadata`/`insert_*`/`load_transcript_messages`/
content-replacement + marble-origami + file-history loaders) and
`SessionManager::{set_title, toggle_tag, save_mode, …}` are thin wrappers
over `append_entries` + `load_entries` + `derive::*`.

### 3.4 Tool-result blobs are a first-class seam *(fixes C1)*

`core/tool-runtime::tool_result_storage` offloads oversized tool-result
**bodies** to `<session_dir>/tool-results/<id>.{txt,json}` and inlines
only a `<persisted-output>` *preview* in the transcript. So the
conversation stays coherent without the blobs, but **full-fidelity
re-fetch is lost** on a host without them → they are *semi-authoritative*
session state, and today they bypass any storage abstraction (raw paths
from `ProjectPaths`). A remote/diskless `RemoteStore` would persist the
transcript and silently drop every blob.

Add a sibling boundary (defined in `coco-tool-runtime`, since that crate
is the producer; injected via `ToolUseContext` like the other handles):

```rust
pub trait SessionBlobStore: Send + Sync {
    fn put_blob(&self, sid: &str, blob_id: &str, mime: &str, bytes: &[u8]) -> Result<()>;
    fn get_blob(&self, sid: &str, blob_id: &str) -> Result<Option<Vec<u8>>>;
    fn list_blobs(&self, sid: &str) -> Result<Vec<BlobMeta>>;
    fn cleanup_older_than(&self, older: Duration) -> Result<i32> { Ok(0) }
}
```

Default `DiskBlobStore` = today's `tool_result_storage` path logic. The
remote backend co-stores blobs with the transcript. **Decision required
(§10):** either blobs travel with the transcript (full recovery) or are
explicitly declared local-only cache with documented degraded re-fetch.
This is Phase **1b** — it crosses into `coco-tool-runtime`, so it ships
adjacent to but separately from the `coco-session` traits.

### 3.5 Durability contract — appends are durable-on-return *(fixes C2)*

The chain pointer makes silent buffering unsafe: `append_message_chain`
returns `ChainWriteResult.last_written_uuid` (storage.rs:813), which the
engine uses as the next turn's `starting_parent_uuid`. If that append is
buffered and lost on crash, the next turn persists an entry whose
`parent_uuid` references a **never-persisted** UUID → the resume DAG walk
(`recovery.rs:171-229`) dangles.

Contract: **every `*Store` write method is durable-on-return for
authoritative backends.**

- **Disk**: `writeln!` (already durable to page cache; add `fsync` only if
  we want crash-durability — separate decision).
- **Remote (authoritative)**: `append_entries` blocks until the backend
  acks. Appends are **once per turn**, so one network round-trip per turn
  is acceptable — and the hot call site at `engine_finalize_turn.rs:1030`
  must move under `spawn_blocking` (harmless for disk; mandatory for
  remote). Same for the rarer async-context writes:
  `engine_prompt.rs:228` (`insert_content_replacement`) and the
  file-history insert (`session_runtime.rs:193`).
- **Mirror leg only** (TeeStore §4.2) may buffer fire-and-forget — it is
  explicitly **lossy / non-authoritative**.

This cleanly separates the two contracts the v1 draft conflated: the
*mirror* is allowed to drop writes; the *authoritative* backend is not.

### 3.6 Resolution & threading *(fixes M1)*

`resolve` must not return a `PathBuf` (meaningless for DB/HTTP). It returns
a backend-agnostic handle:

```rust
pub struct ResolvedSession {
    pub store: Arc<dyn TranscriptIo>,
    pub session_id: String,
    pub project: Option<PathBuf>, // disk worktree origin; None for non-fs
}
```

Disk `SessionCatalog::resolve` keeps the worktree-list fallback internally.
The SDK resume path (`handlers/session.rs:849`) and TUI resume
(`tui_runner.rs:2482-2488`) move onto `spawn_blocking`, so a remote
backend's `block_on` never stalls the executor.

**Threading model (m3):** a remote backend `block_on`s on a **dedicated IO
runtime/handle**, never the shared executor, and its flush task must not
re-enter a sync trait method on its own runtime thread (would panic). State
this in the backend crate's docs.

### 3.7 Error type *(fixes M3)*

`SessionError`'s `Io(io::Error)` / `Json(serde_json::Error)` `#[from]`
variants are disk/json-shaped. Add a backend-neutral variant so a
DB/HTTP backend keeps its `StatusCode` fidelity instead of collapsing to
`Generic`:

```rust
pub enum SessionError {
    Io(std::io::Error),        // disk backend internal
    Json(serde_json::Error),   // disk backend internal
    Backend(coco_error::BoxedError), // remote/DB/HTTP — carries its own StatusCode
    TranscriptNotFound { /* … */ },
    DurationOverflow,
    Generic { message: String },
}
```

## 4. Backends

### 4.1 `DiskTranscriptIo` / `DiskCatalog` / `DiskBlobStore` (default) — Phase 1

Today's code behind the traits: head/tail `summary` override, worktree
fallback in `resolve`, fs locking. `SessionManager::new(memory_base)` keeps
working by building a `DiskCatalog` internally; `with_catalog(Arc<dyn
SessionCatalog>)` is the injection seam.

### 4.2 `TeeStore` (write-mirror decorator) — Phase 2a *(= "Approach B", done right)*

```rust
pub struct TeeStore { primary: Arc<dyn TranscriptIo>, mirror: RemoteSink }
// append → primary durable-on-return (authoritative) + forward to async, LOSSY sink
// load   → primary only
```

Disk stays source of truth; the mirror is best-effort (channel + bg task,
drop-on-backpressure acceptable). Natural sink: the Event Hub
(`hub/connector`, currently a stub). ~80 lines, zero risk to
read/resume/hot-path correctness. Delivers backup / cross-device view /
Hub aggregation **without** committing to remote recovery.

### 4.3 `RemoteStore` (authoritative) — Phase 2b, backend deferred

Same traits against a remote backend (SQL / HTTP / object store — TBD).
Authoritative durable-on-return appends (§3.5); diskless resume via
`load_entries` → `derive::session_resume_state`; co-stores blobs (§3.4).
Disk can become a write-through cache via a `CachingStore` decorator
(§10.2). Pure derivation (§3.1) is reused unchanged.

## 5. Architecture at a glance

```
        consumers (query / cli / coordinator / sdk / tui)
                        │  Arc<dyn TranscriptIo> / Arc<dyn SessionCatalog>
                        ▼
   ┌─────────────────────────────────────────────────────────┐
   │ coco-session: SessionManager + pure derive::* (no IO)     │
   │   summary / session_resume_state / build_message_chain    │
   └───────────────┬───────────────────────────────────────────┘
                   │ traits (sync, Send+Sync, object-safe)
      ┌────────────┼─────────────┬───────────────┐
      ▼            ▼             ▼               ▼
  DiskTranscriptIo TeeStore   RemoteStore   (CachingStore)
  DiskCatalog      = Disk +    = DB/HTTP/    = Remote w/
  DiskBlobStore      lossy        object        disk cache
                     mirror       store
```

`SessionBlobStore` (tool-result bodies) is the parallel seam owned by
`coco-tool-runtime`, injected via `ToolUseContext`.

## 6. Phased plan & call sites

| Phase | Change | Behavior change? |
|---|---|---|
| **0** | Extract `recovery` + metadata derive + chain-build into `storage/derive.rs` over `&[Entry]`; split `TranscriptMetadata` → `SessionSummary` + `StorageStat`. | No (gated by §6.1) |
| **1** | Define the four traits; rename structs → `Disk*`; switch injection to `Arc<dyn …>`; backend-neutral error variant. | No (pure refactor) |
| **1b** | `SessionBlobStore` in `coco-tool-runtime`; `DiskBlobStore` default; route `tool_result_storage` through it. | No |
| **2a** | `TeeStore` decorator → remote sink (backup / Hub). | Opt-in |
| **2b** | `RemoteStore` + optional `CachingStore`; move resume/append call sites under `spawn_blocking`. | Opt-in |

**Phase 1 call sites** (mechanical `Arc<dyn>` swaps): `app/query`
(`engine_finalize_turn.rs:1030`, `engine_prompt.rs:228`,
`engine_builder.rs:294`); `app/cli/session_runtime.rs` (`:84/:193/:1268/
:1271/:1688/:1711`), `session_bootstrap.rs:452`,
`agent_transcript_persistence.rs` (→ adapter over the new trait),
`resume_resolver.rs` (`:82/:116/:193/:207/:222`), `resume_hint.rs:59`,
`headless.rs:821`, SDK `handlers/session.rs:850/:859`,
`tui_runner.rs:2482/:2488`; `coordinator/agent_handle/spawn.rs:1887/:2069`.
The free fns `resolve_session_file_path`/`list_all_sessions` move onto
`SessionCatalog`.

### 6.1 Phase 0 gate — golden equivalence tests *(fixes M4)*

Before deleting the old `recovery`/metadata paths, pin
`SessionResumeState` and `SessionSummary` across a corpus of real
transcripts — including **compact-boundary**, **sidechain**, **plan-slug**,
**marble-origami**, and **>50 MB** cases — asserting byte-identical output
old-path vs new-derive. This is the safety net for the "no behavior
change" claim.

## 7. Approach A vs B — why both, phased

| | **A: trait backend** | **B: write hook only** |
|---|---|---|
| Remote *recovery* (diskless) | ✅ authoritative read path | ❌ one-directional (write-out) |
| Backup / observability / Hub | ✅ via `TeeStore` | ✅ (its sweet spot) |
| Consistency | authoritative (durable-on-return) | best-effort, lossy → backup only |
| Local-disk dependency removable | ✅ | ❌ reads still need local files |
| Cost / risk | real refactor; async is a backend impl detail | cheap, near-zero risk |
| Promotable | — | ❌ if bolted onto fs fns; ✅ only if it *is* the trait |

**B is a special case of A.** As a trait decorator (`TeeStore`) the mirror
is one impl and promotable; bolted onto the fs functions it is a dead-end
parallel path. Hence: land A's traits + disk default (no behavior change),
ship `TeeStore` for backup/Hub, add `RemoteStore` for diskless recovery.

## 8. Risks / watch-items

- **Hot-path blocking** (§3.5): wrap the per-turn append + the two other
  async-context writes under `spawn_blocking` before wiring a remote
  backend; never do sync network IO on the executor.
- **`summary` semantics** (§3.2): the head/tail window is a perf hack, not
  a contract; default derives from full entries. Snapshot tests pin
  derived output, not bytes.
- **`append_entries` atomicity** (m2): disk = sequential `writeln!`,
  partial-last-line tolerated by the reader; DB = one transaction per
  batch. Pin in the trait doc.
- **`store_for` cost** (m1): cheap for disk (`ProjectPaths`); a remote
  backend must pool connections, not reconnect per call.
- **Blob recovery** (§3.4): unresolved authoritative-vs-cache decision; do
  not ship `RemoteStore` claiming full recovery until blobs are covered.
- **Naming/namespace** (m4): reconcile the new `AgentTranscriptStore`
  trait with the existing cli async one — one abstraction, not two.

## 9. Out of scope (keep local)

- **`SessionRegistry`** (PID files, `libc::kill`): host/PID-bound liveness.
  Fleet liveness needs a lease/heartbeat model owned by the Event Hub.
- **`PromptHistory`**: config_home input recall, `fs2`-locked. Same trait
  pattern applies later if wanted; low ROI now.

## 10. Open questions

1. Remote backend target (SQL / HTTP-Hub / object store) — deferred; any
   preferred direction to design `RemoteStore` toward?
2. Under `RemoteStore`, is disk a write-through cache (`CachingStore`) or
   does it go fully remote?
3. ✅ **Resolved + shipped:** backend selection is a resolved `RuntimeConfig`
   field `session.backend` (`disk` default + a pure-RAM `memory` backend;
   authoritative `RemoteStore` still deferred). No new `COCO_*` env.
4. ✅ **Resolved:** tool-result blobs are **local-only** disk cache; the
   truncated `<persisted-output>` preview already travels in the transcript,
   so remote recovery is preview-fidelity by design (full re-fetch degrades
   without the local blob). No `SessionBlobStore` this iteration.
5. Disk crash-durability: is `fsync`-on-append wanted, or is page-cache
   durability sufficient (current behavior)?

## 11. Non-goals recap

No concrete remote backend; no PID-registry / prompt-history remoting; no
cross-host format importer. Disk remains the default and the only backend
that must exist after Phases 0–1.

## 12. Changelog (v1 → v2)

v2 folds in the adversarial review:

- **C1** added `SessionBlobStore` — tool-result bodies were bypassing the
  seam entirely (§3.4).
- **C2** durability is now a first-class contract: appends durable-on-return
  for authoritative backends; only the mirror buffers (§3.5).
- **C3** split `TranscriptMetadata` → `SessionSummary` (derived) +
  `StorageStat` (backend); honest `summary` default; drop the 50 MB hard
  refusal in favor of streaming (§3.2).
- **M1** backend-agnostic `ResolvedSession` (no `PathBuf`); SDK/TUI resume
  moved off the executor (§3.6).
- **M2** one fat trait → `TranscriptIo` / `AgentTranscriptStore` /
  `UsageSnapshotStore` / `SessionCatalog` (ISP); subsumes the existing cli
  agent trait (§3.3).
- **M3** `SessionError::Backend(BoxedError)` for non-fs backends (§3.7).
- **M4** golden equivalence tests as the explicit Phase 0 gate (§6.1).
- **m1–m4** captured as watch-items (§8).
