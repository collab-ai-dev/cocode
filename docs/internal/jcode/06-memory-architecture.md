# Memory Architecture (Semantic / Passive Recall): jcode vs coco-rs

> Source-level comparison. Every claim below was checked against the actual
> Rust on both sides; file:line refs are load-bearing. Where a README or a
> design doc oversold a feature, the source verdict is stated plainly.
>
> The two harnesses descend from different lineages. jcode is an independent
> agent that built a *passive semantic-recall* memory from scratch. coco-rs
> is a faithful port of Claude Code's *file* memory (model-curated markdown +
> an LLM ranker). A capability jcode has and coco-rs lacks is therefore not
> automatically a coco-rs defect — it is judged on engineering merit for
> coco-rs's stated goals (multi-provider neutrality, low RAM at rest,
> TS-observable parity).

---

## jcode approach

jcode implements genuine **passive recall**: relevant memories surface from a
local embedding search gated by a small-LLM ("sidecar"), delivered to the main
agent **one turn behind** so the main loop never blocks. The pipeline
(`src/memory_agent.rs:394-644`, `process_context`):

1. **Repeat-suppression gate** — unchanged context within 30s skips the
   relevance check (`RELEVANCE_CONTEXT_REPEAT_SUPPRESSION_SECS=30`,
   `memory_agent.rs:251,407-424`).
2. **Embed off-thread** — `tokio::task::spawn_blocking(|| embedding::embed(...))`
   (`memory_agent.rs:435`).
3. **Topic-change detection** — cosine of the new context vs the session's
   last embedding; below `TOPIC_CHANGE_THRESHOLD=0.3` (`memory_agent.rs:42,466`)
   it extracts the *previous* topic (gated by `MIN_TURNS_FOR_EXTRACTION=4`,
   `:244,478`) and clears the surfaced set (`:486-498`).
4. **Periodic extraction** — every `PERIODIC_EXTRACTION_INTERVAL=12` turns when
   context ≥200 chars, a single-topic safety net (`:248,513-526`).
5. **Embedding search** — `find_similar_with_embedding(ctx_emb,
   EMBEDDING_SIMILARITY_THRESHOLD=0.5, EMBEDDING_MAX_HITS=10)` over per-memory
   **content** vectors (`memory_agent.rs:531-535`; consts `memory.rs:1807,1810`).
6. **Filter already-surfaced / already-injected** (`memory_agent.rs:548-559`).
7. **Parallel sidecar relevance** — each survivor is verified by a small-LLM
   `check_relevance` run via `join_all` (`memory_agent.rs:581-583`, sidecar call
   at `:686-691`). The comment notes the embedding pre-filter amortizes this to
   "1-5 calls instead of 30" (`memory.rs:1168-1173`).
8. **Format + stash** as `PENDING_MEMORY` (`set_pending_memory_with_ids_and_display`,
   `memory_agent.rs:624-630`).
9. **Spawn post-retrieval maintenance** (`memory_agent.rs:640`).

**Embedding layer (real, not marketing).** `crates/jcode-embedding/src/lib.rs:9`
loads `all-MiniLM-L6-v2` via `tract-onnx` + `tokenizers`, 384-dim, mean-pooled
+ L2-normalized; `cosine_similarity` / `batch_cosine_similarity` / `find_similar`
(top-k heap) at `lib.rs:202-252`. The facade `src/embedding.rs` adds a
process-wide cache, a 128-entry LRU keyed by text hash, and idle-unload with
`malloc_trim`/jemalloc purge. This is a self-contained semantic search engine
with no browser/TS dependency — the README claim holds in source.

**Memory model.** `crates/jcode-memory-types/src/lib.rs:232-396` defines
`MemoryEntry` with `MemoryCategory` (Fact/Preference/Entity/Correction/Custom),
`TrustLevel`, `strength`, `Vec<Reinforcement>` breadcrumbs, an
`embedding: Option<Vec<f32>>` **persisted on the entry** (so cosine is computed
against a stored corpus, not re-embedded per write), and a `confidence`.
`effective_confidence()` (`lib.rs:318-334`) applies **time-based exponential
decay with category-specific half-lives** (Correction 365d, Preference 90d,
Fact 30d, Entity 60d) plus a log-access boost. `boost_confidence` /
`decay_confidence` (`lib.rs:336-346`) tune it on use.

**Graph store.** `crates/jcode-memory-types/src/graph.rs` is the actual
`MemoryGraph` — **HashMap-based, not petgraph** (`graph.rs:229-256`; the doc's
own Phase-4 line admits "HashMap-based … simpler than petgraph"). It holds
`memories`, `tags`, `clusters`, forward `edges` and `reverse_edges` (O(1)
incoming lookup). `EdgeKind` = HasTag/InCluster/RelatesTo{weight}/Supersedes/
Contradicts/DerivedFrom with `traversal_weight()` (`graph.rs:114-126`).
`cascade_retrieve()` (`graph.rs:546-618`) does BFS from embedding seeds,
depth-2, score = `seed × edge_weight × 0.7^depth`.

**Write-time consolidation.** Storage-layer dedup at cosine **≥0.85**
(`STORAGE_DEDUP_THRESHOLD`, `memory.rs:322`) with a **cross-store** check into
the other graph (project↔global), reinforcing instead of duplicating
(`memory.rs:332-395`). Incremental extraction independently dedups at **≥0.90**
(`memory_agent.rs:820-874`), runs a sidecar **contradiction check** within a
category and on a hit deterministically `supersede()`s the old memory + adds a
`Contradicts` edge (`memory_agent.rs:876-934`), and adds `DerivedFrom` edges
between co-extracted memories (`:942-960`).

**Post-retrieval maintenance** (`memory_agent.rs:994-1125`, fully spawned):
`RelatesTo` link discovery between co-relevant pairs; **confidence boost +0.05
on verified / decay −0.02 on rejected** (`:1040-1052`); **gap detection** — a
`MaintenanceGap` event when candidates existed but none verified
(`:1056-1065`); cluster refinement every `CLUSTER_REFINEMENT_INTERVAL=50`
retrievals (`:51,1069`); low-confidence pruning (conf<0.15 & age≥24h).

**Cross-turn delivery & dedup** (`src/memory/pending.rs`): `is_fresh()` = <120s
(`pending.rs:55-57`). `take_pending_memory` applies, in order, staleness discard,
**prompt-signature suppression within 90s** (`MEMORY_REPEAT_SUPPRESSION_SECS`,
`pending.rs:30,94-105`), and **memory-set overlap suppression ≥0.8 within 180s**
(`MEMORY_SET_OVERLAP_SUPPRESSION_RATIO=0.8`, `MEMORY_SET_REPEAT_SUPPRESSION_SECS=180`,
`pending.rs:32-35,107-125`) before injecting; injected IDs are marked so they
do not re-surface — but surfaced sets re-open every `TURN_RESET_INTERVAL=50`
turns (`memory_agent.rs:48,373-380`) and on topic change, so a memory *can*
re-surface later.

**Observability.** `src/memory_log.rs` writes dated JSONL
(`~/.jcode/logs/memory-events-YYYY-MM-DD.jsonl`, 14-day retention) covering
embedding latency/hits (`memory_log.rs:109-115`), candidate-filter counts
(`:347-363`), sidecar verdicts (`:117-133`), injection, and maintenance/gap
events — driving a live 4-step pipeline TUI widget.

**`conversation_search` tool** (`src/tool/conversation_search.rs`) is
**keyword-only substring search** over a transcript + stats/turn-range
retrieval (`conversation_search.rs:18,116,258`) — NOT embedding-backed despite
sitting near the memory system.

**README verdict.** (a) Local-embedding semantic memory + memory graph +
ambient consolidation: **substantiated in source.** (b) The marquee
"**27.8 MB RAM**" is explicitly **"local embedding off"** (`README.md:71`);
with embeddings **on** it is **167.1 MB / 6.0× more** (`README.md:77-78`) — i.e.
*the module under comparison is jcode's single largest RAM line item, opt-in,
and excluded from the headline number*, with idle-unload as the mitigation.
(c) The architecture doc oversells maturity: **petgraph DiGraph**, **HDBSCAN
clustering**, **negative/procedural memories**, **temporal awareness**, and deep
graph-wide consolidation ("Ambient Garden") are unimplemented TODOs
(`docs/MEMORY_ARCHITECTURE.md:739-743,761-773`).

---

## coco-rs approach

coco-rs ports Claude Code's **file** memory: the model writes human-readable
markdown memory files (one topic per file) into a per-project memdir, with a
model-curated `MEMORY.md` pointer index. Recall surfaces them via an **LLM
ranker**, not embeddings. The memory crate (`coco-rs/memory/`) has **zero
embedding/vector/cosine code** — verified: `memory/Cargo.toml` declares no
fastembed/onnx/tract dependency.

**4-type taxonomy + frontmatter.** Closed enum `MemoryEntryType`
User/Feedback/Project/Reference (`store/types.rs`); each file carries
`name/description/type` frontmatter; the `description` is what the ranker reads.
`MEMORY.md` is **model-curated and never auto-regenerated** — the runtime only
reads + truncates it (200-line / 25 KB caps via `EntrypointTruncation`).

**Recall** (`memory/src/recall.rs`, `runtime.rs:661-859`). Per turn the engine
calls `MemoryRuntime::recall(query, recent_tools)`, wired at
`app/query/src/reminder_adapters.rs:351-378`. Flow:

- Cheap pre-gates (`runtime.rs:681-707`): empty/single-token query, 60 KB
  session byte budget exhausted, or empty scan → return early.
- `scan_memory_files` (200-cap, 30-line frontmatter read, mtime-desc).
- `build_selection_prompt` (`recall.rs:138-164`) feeds the ranker only
  `[type] filename (ts): description` via `format_memory_manifest` — **bodies
  are never sent**.
- A **`ModelRole::Memory` side-query** asks for `{selected_memories: [≤5
  filenames]}`. Two-attempt strategy: native structured-output when the resolved
  model declares `Capability::StructuredOutput` (`runtime.rs:755-792`), else a
  forced-tool fallback (synthetic `select_memories`, `runtime.rs:794-838`). An
  empty `selected_memories: []` is treated as a legitimate "no matches" verdict,
  not malformed (`extract_recall_selection`, `recall.rs:289-324`), sparing a
  wasted 2nd call.
- Selected files are loaded with per-file **4 KB truncation** (`MAX_BODY_BYTES`,
  `recall.rs:27`), **freshness headers** for >1-day-old files
  (`memory_freshness_text`, `recall.rs:370`), a hard **60 KB per-session byte
  budget** (`MAX_SESSION_BYTES`, `recall.rs:31`), and a **`PrefetchState`**
  already-surfaced dedup (`recall.rs:46-98`).
- When no ranker handle is installed or it errors, recall **stays silent**
  (`recall.rs:384-390`, `runtime.rs:709-712`) — TS parity; the recency
  heuristic was *deliberately removed*. (Note: the comment at
  `reminder_adapters.rs:357-360` still mentions "the recency heuristic"; it is
  stale — the code path returns empty.)

**Auto-extraction** (`service/extract.rs`). After every eligible turn the
engine forks a **subagent** (`isolation="fork"` + `fork_context_messages`,
`max_turns=5`, memdir-only write fence); it reads existing memories (manifest
pre-injected) and writes/edits files for the slice since a cursor. Gates:
`extraction_enabled` → `Feature::AutoMemory` → coalesce → skip+advance if the
main agent already wrote to memory → throttle (`extraction_throttle`, default 1,
`extract.rs:398-414`) → cursor advance on success. There is **no embedding
similarity check** in `extract.rs` (verified).

**Auto-dream consolidation** (`service/dream.rs`). Background forked subagent
that merges related entries, resolves contradictions, and prunes stale pointers
via a verbatim 4-phase prompt (`prompt/text/dream.md` Orient/Gather/Consolidate/
Prune; contradiction resolution is *model judgment* at `dream.md:46`). 3-gate
scheduler: time (`dream_min_hours`, default 24, `dream.rs:252`) → scan-throttle
(`SCAN_THROTTLE=10 min`, `dream.rs:34,270`) → session gate (`dream_min_sessions`,
`dream.rs:285`) → PID+mtime CAS lock with mtime rollback on failure.

**Session memory** (`service/session.rs`). A 9-section structured summary
persisted to `<config_home>/session-memory/<id>.md` (mode 0o600) so context
survives compaction/`--resume`. Trigger gates: token growth ≥5 K (or ≥10 K
first time) AND activity (≥3 tool calls or natural break).

**Security & paths.** Memdir anchored to `coco_git::find_canonical_git_root`.
`path/validate.rs` rejects null bytes/UNC/drive-root/tilde/fullwidth/
URL-encoded `../`/backslash absolutes; a `realpath_deepest_existing` symlink
walk guards the write fence. Defense-in-depth `can_use_tool` per-fork policy
(`can_use_tool.rs`): Read/Glob/Grep free, Bash only if
`coco_shell_parser::safety::is_known_safe_command` and no metachars, Write/Edit
only inside memdir.

**Team sync + telemetry.** `team_sync/` implements HTTP delta push/pull
(etag/304, sha256, 200 KB batch cap) + a secret scanner + file-watch trigger.
`telemetry.rs` maps each `MemoryEvent` to a TS `tengu_*` event; the enum covers
`MemdirLoaded`/`Extraction*`/`AutoDream*`/`SessionMemory*`/`KairosRollover` but
has **no recall-selection variant** (verified — no `Recall*` in `telemetry.rs`).

**Separate (not memory): `coco-retrieval`.** coco-rs *does* own a real semantic
engine — `coco-retrieval` has fastembed local ONNX (nomic/bge/MiniLM, 384/768-dim,
`embeddings/fastembed.rs`), OpenAI embeddings (`embeddings/openai.rs`), a batched
embedding queue + LRU cache (`embeddings/queue.rs`, `cache.rs`), `sqlite_vec`/
LanceDB vector stores, BM25, a `HybridSearcher` with RRF fusion, a reranker, and
a PageRank repo-map. Its `EmbeddingProvider` trait lives at
`retrieval/src/traits.rs:43` (`embed` / `embed_batch`). **But it is scoped to
CODE retrieval and is NOT wired into the memory crate** — a separate subsystem
gated behind `Feature::Retrieval` (`common/types/src/features.rs:121`).

---

## Head-to-head comparison

| Axis | jcode | coco-rs |
|------|-------|---------|
| **Recall match basis** | Embedding cosine over memory **bodies** + sidecar verify (`memory_agent.rs:531-535`) | LLM rank over **descriptions** only (`recall.rs:138-164`) |
| **Recall placement** | Off-thread, **one turn behind** via `PENDING_MEMORY` (`pending.rs`) | **Synchronous side-query** inside the turn's reminder phase (`reminder_adapters.rs:366`) |
| **Recall cost** | ≤5 sidecar calls only on embedding hits; amortized | 1 LLM round-trip per qualifying turn on the hot path |
| **Write-time dedup** | Deterministic cosine ≥0.85/0.90 → reinforce (`memory.rs:322-395`) | Prompt instruction only (`builders.rs:253`) + 24 h dream pass |
| **Contradiction** | Sidecar check → deterministic `supersede()` + edge (`memory_agent.rs:876-934`) | Model judgment in the 24 h dream (`dream.md:46`) |
| **Confidence / decay** | Per-entry confidence, category half-lives, boost/decay on use (`lib.rs:318-346`) | None — freshness is file mtime (`recall.rs:370`) |
| **Associative structure** | Typed-edge graph + BFS cascade (`graph.rs:546-618`) | Flat file set + flat `MEMORY.md` |
| **Recall observability** | Per-candidate JSONL (latency/hits/verdict/gap) (`memory_log.rs`) | Coarse `MemoryEvent`; **no recall-selection event** |
| **RAM at rest (memory)** | +~140 MB with embeddings on (27.8→167.1 MB, `README.md:71-78`) | Near-free (no model, no vector store) |
| **Provider neutrality** | Sidecar = fixed small-model concept baked into call sites | All calls via `ModelRole::Memory` — operator picks provider |
| **Write isolation** | In-process (`extract_from_context` writes via `MemoryManager`) | Forked subagent, `max_turns=5`, memdir-only fence |

**The core difference.** jcode's two-stage pipeline (cheap-local-recall →
small-LLM-verify, off-thread, one-turn-behind) is both **more recall-complete**
(semantic match over bodies surfaces memories whose wording differs from the
query) and **lower perceived latency** than coco-rs's single synchronous
text-rank on descriptions. A coco-rs memory whose body is relevant but whose
one-line `description` doesn't match the query is invisible to the ranker.
coco-rs even concedes it has no fallback when the ranker is unavailable — it
goes silent (`recall.rs:384-390`).

For a low-RAM, multi-provider tool that already pays for LLM calls, coco-rs's
trade is *defensible* — but on raw recall capability, embed-and-verify is
strictly more capable than rank-the-description.

---

## Where coco-rs already matches or wins

1. **A more capable semantic engine — just aimed elsewhere.** jcode's stack is
   a single MiniLM-384 model + a hand-rolled top-k heap
   (`jcode-embedding/src/lib.rs`). coco-rs's `coco-retrieval` is a *superset*:
   multiple embedding models (`embeddings/fastembed.rs`) **plus** OpenAI
   embeddings, a real vector store (`storage/sqlite_vec.rs`; LanceDB option),
   BM25, **hybrid RRF fusion** (`search/hybrid.rs`), a neural/remote reranker,
   AST chunking, and a PageRank repo-map. jcode has nothing comparable to RRF
   fusion or a reranker. The capability exists in-tree; wiring it into memory is
   a *seam* task, not a missing-engine task.

2. **Memory security & path hardening: coco-rs is clearly ahead.** coco-rs
   validates null/UNC/drive-root/tilde/fullwidth/URL-encoded-`../`/backslash and
   does a `realpath_deepest_existing` symlink-escape walk (`path/validate.rs`,
   `path/symlink.rs`), plus a **two-ring write fence** (per-fork `can_use_tool` +
   `allowed_write_roots`, `can_use_tool.rs`). jcode's memory writes go through
   `MemoryManager` with a hashed-cwd path and **no path-traversal/symlink
   validation layer and no per-write tool fence**. jcode's memory is
   internal-only so the threat model differs, but the hardening gap is real.

3. **Secret-redaction before persistence.** coco-rs runs `coco_secret_redact` in
   `team_sync::secret_scanner` and the auto-mem fence. jcode's "do not remember
   secrets" is a **doc/privacy section** (`MEMORY_ARCHITECTURE.md:780-797`) — no
   executed secret scanner in the memory write path; avoidance relies on the
   extraction LLM's judgment.

4. **Cleaner layering / multi-provider neutrality.** coco-rs memory never
   hardcodes a model — recall/extract/dream all go through `ModelRole::Memory` /
   `AgentHandle` side-queries, so the operator picks provider+model. jcode's
   sidecar is a fixed small-model concept baked into `Sidecar::new()` call sites;
   switching providers for the memory gate is not a config knob in the memory
   code.

5. **Forked-subagent extraction with a real write fence.** coco-rs's extraction
   runs as an isolated forked agent (`max_turns=5`, memdir-only writes), so
   runaway loops or prompt-injection in the transcript are bounded. jcode's
   `extract_from_context` runs inline in the memory-agent task and writes
   directly via `MemoryManager` with no turn/scope sandbox.

**jcode claims that do NOT hold in source:** petgraph DiGraph (actual:
HashMap, `graph.rs:229-256`); HDBSCAN clustering (actual: co-relevance centroid
clusters only); negative/procedural memories + temporal awareness (unchecked
TODOs, `MEMORY_ARCHITECTURE.md:739-743`; `MemoryCategory` has no such variant);
"Ambient Garden" deep consolidation (unimplemented). The headline 27.8 MB is
"embedding off"; embeddings-on is 167.1 MB / 6.0× (`README.md:71-78`).

---

## Optimization recommendations for coco-rs (adversarially verified)

All six analyst suggestions survived adversarial review (5 confirmed, 1
nuanced). None conflict with a documented coco-rs non-goal: embedding-assisted
memory is gated behind the existing `Feature::Retrieval` so the default build
stays embedding-free, and the recommendations are framed as *additive* layers on
top of the TS-faithful behavior, not replacements. Strong verifier
missed-findings are folded in as M06-S7 … M06-S9.

### M06-S1 — Deterministic write-time dedup via the existing retrieval embedder (confirmed)

**Why.** jcode prevents near-duplicate memories at write time with an embedding
check (cosine ≥0.85 storage-layer with cross-store reinforce, `memory.rs:322-395`;
≥0.90 incremental reinforce-not-store, `memory_agent.rs:820-874`), against a
*persisted* per-entry vector (`jcode-memory-types/src/lib.rs:262`). coco-rs
delegates dedup entirely to a prompt instruction ("Do not write duplicate
memories. First check…", `builders.rs:253`); `extract.rs` has no similarity
logic and `memory/Cargo.toml` no embedding dep. The only structural dedup is the
once-per-24 h dream pass — between dreams a model that ignores the instruction
accretes near-duplicate files.

**Concrete change.** In `coco-memory`, add an optional dedup gate to
`ExtractService` (and `DreamService`) that, when `Feature::Retrieval` is
enabled, embeds candidate memory descriptions/bodies via the existing
`EmbeddingProvider` (`retrieval/src/traits.rs:43`, `embeddings/fastembed.rs` or
`openai.rs`) and rejects/merges writes whose cosine to an existing memory exceeds
~0.9. **Inject the provider as `Option<Arc<dyn EmbeddingSimilarity>>` on
`MemoryRuntimeBuilder`** so memory (L4) gains no hard dependency on retrieval
(standalone); no-op when absent.

**Impact** high · **Effort** medium · **Risk** memory crate must not gain a
mandatory retrieval dep — mitigate with the injected trait object. Threshold
tuning needed so legitimately-distinct memories aren't suppressed.

### M06-S2 — Rank recall over memory BODIES via an embedding pre-filter (confirmed)

**Why.** jcode recall matches the full corpus by meaning (cosine over stored
**content** vectors, `memory_agent.rs:531-535`, threshold 0.5 at `memory.rs:1807`)
and surfaces memories whose wording differs from the query. coco-rs's
`build_selection_prompt` (`recall.rs:138-164`) feeds the ranker only the
one-line `description`; bodies are never embedded or ranked, and the non-LLM
fallback was removed (`recall.rs:384-390`). A memory with a relevant body but a
mismatched description is invisible.

**Concrete change.** Add an optional embedding pre-filter to
`MemoryRuntime::recall`: when an `EmbeddingProvider` is wired
(`Feature::Retrieval`), embed the query + memory bodies (cache by file mtime via
`coco-retrieval`'s `embeddings/cache.rs`) and shortlist top-N by cosine **before**
handing the shortlist to the existing LLM ranker. This **preserves the
LLM-rank-of-5 contract** (TS parity) but feeds it a body-relevant candidate set;
falls back to the description-only manifest when no provider is present. Keep the
60 KB session byte budget intact.

**Impact** high · **Effort** medium · **Risk** must preserve TS-parity recall
shape when embeddings are off (no default-build behavior change); reuse
retrieval's queue/cache rather than rolling a new memdir index.

### M06-S3 — Take recall off the turn's serial critical path (nuanced — correction folded in)

**Why.** jcode runs recall fully off the main loop (`update_context_sync` via
non-blocking `try_send` on a 16-deep channel, `memory_agent.rs:223`) and delivers
one turn behind via `PENDING_MEMORY` / `take_pending_memory` (`pending.rs:85-142`,
`is_fresh<120s`). coco-rs awaits `runtime.recall(...)` **inline** in the reminder
phase (`reminder_adapters.rs:366`), which issues a full `ModelRole::Memory`
round-trip (`runtime.rs:772`/fallback `815`) **before** the main streaming call —
so every qualifying turn pays a synchronous extra LLM round-trip up front.

**Correction (from adversarial review — do not overstate the existing scaffold):**
- The comments calling this "async prefetch" (`generators/memory.rs`,
  orchestrator naming) are **aspirational**, mirroring TS
  `startRelevantMemoryPrefetch`. **There is no spawned task today** — a prefetch
  model must be *built*, not toggled.
- `recall::PrefetchState` (`recall.rs:46-98`) is **only** the surfaced-paths +
  byte-budget tracker; it is **not** a stashed future. A new spawn+stash
  mechanism (e.g. a `JoinHandle`/`oneshot` keyed by turn) is required.
- TS itself prefetches-then-awaits *in the same turn*, not strictly N+1.

**Concrete change (two tiers).**
- **(a) Cheap / TS-faithful:** in `coco-query`, spawn `runtime.recall()`
  *concurrently* with `build_prompt` + `build_tool_definitions` and `join` before
  the API call. Overlaps the round-trip with prompt assembly; no observable
  behavior change.
- **(b) jcode-style N+1:** add a real prefetch task that stashes turn N's result
  for injection at N+1, with a 120 s freshness guard, **gated behind a config
  flag (`recall_prefetch`) defaulting to current synchronous behavior** to keep
  strict TS parity available. First turn after `/clear` has no prefetched recall.

**Impact** medium · **Effort** medium · **Risk** tier (b) changes *when* a memory
appears (one turn later) — a TS divergence, hence the flag. Do **not** claim
`PrefetchState` already provides the stash.

### M06-S4 — Topic-change detection to trigger mid-session extraction (confirmed)

**Why.** jcode detects topic shifts (cosine of consecutive context embeddings <
`TOPIC_CHANGE_THRESHOLD=0.3`, `memory_agent.rs:42,464-466`) and on a shift
extracts the *previous* topic (gated by `MIN_TURNS_FOR_EXTRACTION=4`, `:478`) and
clears the surfaced set (`:486-498`), capturing learnings before the thread moves
on. coco-rs's `ExtractService` gates only on `extraction_throttle` + cursor
(`extract.rs:398-414`) and `SessionMemoryService` only on token-growth; neither
uses a topic-boundary signal, and `PrefetchState` resets only on `/clear`
(`recall.rs:93-97`).

**Concrete change.** When an `EmbeddingProvider` is available, have the engine
cosine the current vs previous user-turn embedding and, on a sub-threshold drop,
(a) force an `ExtractService` run for the prior slice and (b) call
`PrefetchState::reset()` so the new topic can re-surface. When embeddings are off,
approximate with a lexical-overlap heuristic. **Purely additive** — only adds
extraction triggers, never suppresses a TS-mandated one. Gate behind
`Feature::Retrieval` / a config toggle.

**Impact** medium · **Effort** medium · **Risk** extra forks cost LLM calls —
bound with the existing throttle + a min-turns-since-extraction guard (jcode uses
`MIN_TURNS_FOR_EXTRACTION=4`).

### M06-S5 — Per-session recall repeat/overlap suppression on the injection path (confirmed)

**Why.** jcode suppresses re-injecting the same/overlapping memory set:
prompt-signature dedup within 90 s, ≥0.8 memory-set overlap within 180 s, marks
injected IDs (`pending.rs:30-35,85-142`), but re-opens surfaced sets every 50
turns / on topic change so memories *can* re-surface. coco-rs's `PrefetchState`
tracks only an `already_surfaced: HashSet<path>` + cumulative bytes
(`recall.rs:51-54,75-83`): **no signature suppression, no overlap-ratio check, no
time decay**. Worse, the failure mode is *opposite* — once a path is surfaced it
is **permanently** skipped (`recall.rs:341`) until `/clear`, so a newly-relevant
memory can never re-surface.

**Concrete change.** Extend `recall::PrefetchState` with (a) a last-injected
memory-set + timestamp and an overlap-ratio suppression window (mirror jcode's
0.8/180 s), and (b) optionally allow a surfaced memory to **re-surface after a
cooldown** rather than never (jcode resets every 50 turns / on topic change).
Pure `coco-memory` change — no cross-crate seam, no embedding dep. Keep the
60 KB budget and `MAX_RELEVANT=5` caps.

**Impact** medium · **Effort** low · **Risk** low — tuning only. Re-surface-after-
cooldown is a slight TS divergence (gate behind a constant if strict parity is
required); the overlap-suppression half is strictly safer than today.

### M06-S6 — Fine-grained recall telemetry (confirmed)

**Why.** jcode logs every recall mechanic — `embedding_complete{latency_ms,hits}`
(`memory_log.rs:109-115`), `candidate_filter{total,after_dedup}` (`:347-363`),
sidecar verdicts (`:117-133`), and `MaintenanceGap` when candidates exist but none
verified (`memory_agent.rs:1056-1065`) — making "why didn't memory X surface"
diagnosable. coco-rs's `MemoryEvent` (`telemetry.rs`) covers extraction/dream/
session lifecycle but **has no recall-selection variant**; `MemoryRuntime::recall`
emits zero telemetry (only `tracing::debug` on malformed/fallback). The doc
explicitly defers it: "`tengu_memory_recall_shape` … Skipped to keep telemetry
surface minimal; reintroduce if recall quality needs measurement"
(`crate-coco-memory.md:377`).

**Concrete change.** Add `MemoryEvent::RecallRanked { scanned, manifest_entries,
selected, malformed_fallback, latency_ms }` (and optionally `RecallGap { scanned,
selected: 0 }`) emitted from `MemoryRuntime::recall`. Keep payload
**metadata-only** (counts/latency, no memory content) to respect the L3
redaction rule. This is the prerequisite signal for tuning S2/S5 — and is
consistent with the doc's own "reintroduce if recall quality needs measurement"
note.

**Impact** low · **Effort** low · **Risk** minimal.

---

### Additional recommendations from verifier missed-findings

These are real jcode mechanisms the analyst's six did not isolate. They share
the S1/S2 embedding seam and the same `Feature::Retrieval` gating, so they should
be planned together rather than as one-off patches.

### M06-S7 — Write-time deterministic contradiction → supersede (verifier)

**Why.** jcode runs a sidecar `check_contradiction` on each new memory against
same-category candidates and, on a hit, deterministically `supersede()`s the old
memory + adds a `Contradicts` graph edge (`memory_agent.rs:876-934`). coco-rs
resolves contradictions **only** via model judgment during the 24 h dream pass
(`dream.md:46`) — no write-time deterministic supersede; a stale fact can stay
authoritative for up to a day.

**Concrete change.** Fold into the S1 dedup gate: when the embedding pre-filter
finds a high-similarity same-type memory whose *content* the new write
contradicts, have the extraction fork prefer an **Edit** of the existing file
over a new write (the model already has Edit in-fence). A fully deterministic
supersede would need a confidence/edge model coco-rs intentionally lacks (see
"Rejected" below), so this is the file-memory-shaped version: bias toward update,
surfaced by the S1 similarity signal.

**Impact** medium · **Effort** medium (rides S1) · **Risk** low; additive,
`Feature::Retrieval`-gated.

### M06-S8 — Periodic single-topic extraction safety net (verifier)

**Why.** Independent of the S4 topic shift, jcode force-extracts every
`PERIODIC_EXTRACTION_INTERVAL=12` turns when context ≥200 chars
(`memory_agent.rs:248,513-526`), guaranteeing capture during long *same-topic*
sessions. coco-rs extraction fires only on the throttle counter
(`extract.rs:398`) with no turn-count safety net, so a long single-topic session
that never trips the throttle's other conditions can under-extract.

**Concrete change.** Add a turn-count safety net to `ExtractService`: force an
extraction run after N turns since the last successful extraction even if other
gates haven't fired. This needs **no embedding** and is a pure `coco-memory`
change — pair it with S4 (topic-shift = event trigger; periodic = time-out
trigger). Tune N against the existing throttle to avoid double-firing.

**Impact** medium · **Effort** low · **Risk** low; additive, bounded by the
existing throttle.

### M06-S9 — A structured conversation-history retrieval tool (verifier)

**Why.** jcode ships `ConversationSearchTool` — keyword search + turn-range fetch
+ stats over compacted history (`src/tool/conversation_search.rs`). coco-rs's
`searching_past_context` is **only a prompt instruction** telling the model to
grep the transcript dir (`builders.rs:160-161`, `text/searching_past_context.md`)
— there is no structured retrieval tool over past conversation.

**Concrete change.** This is a *tool* addition (L3 `coco-tools`), not a memory
change: a keyword/turn-range/stats tool over the session transcript store would
give the model a deterministic handle on its own history instead of relying on
ad-hoc grep. Optionally back it by `coco-retrieval`'s BM25 (`search/bm25.rs`) for
ranked rather than substring hits — but note jcode's own tool is **keyword-only**
(`conversation_search.rs:18,258`), so a substring/BM25 first cut is already at
parity. Scope it outside the memory crate.

**Impact** low–medium · **Effort** medium · **Risk** low; a new gated tool, no
behavior change to existing flows.

---

## Rejected after adversarial review

No analyst suggestion (M06-S1 … M06-S6) was refuted — all six passed (S3 with a
correction, folded above). What was *checked and dropped* are the deeper jcode
mechanisms that conflict with coco-rs's documented design choices:

- **Port the full typed-edge `MemoryGraph` + per-entry confidence/decay model**
  (`graph.rs`, `jcode-memory-types/src/lib.rs:318-346`). Rejected as a *design*
  goal: coco-rs's memory is deliberately a **flat directory of model-curated
  frontmatter files** with a model-curated `MEMORY.md` — a faithful port of
  Claude Code's file memory. A graph + numeric confidence is a different memory
  model, not a parity gap. The *associative* benefit jcode gets from edges is
  partly recoverable through the S2 body-embedding pre-filter (relatedness by
  meaning at recall time) without introducing a second persisted store the model
  doesn't author. M06-S7 captures the *contradiction* slice in a file-shaped way.

- **A persisted per-memory embedding column on the memory file** (jcode stores
  `embedding: Option<Vec<f32>>` on every `MemoryEntry`, `lib.rs:262`). Rejected
  as stored *state*: it would make `coco-memory` own a vector store and break the
  "memory files are human-readable, model-curated" invariant. The S1/S2
  recommendations instead **cache** body embeddings in `coco-retrieval`'s existing
  mtime/content-hash cache (`embeddings/cache.rs`), keyed off the file — derived,
  disposable, and rebuildable, not a new source of truth in the memdir.

- **HDBSCAN clustering + "Ambient Garden" deep consolidation.** Not portable
  because they are **not implemented in jcode either** (unchecked TODOs,
  `MEMORY_ARCHITECTURE.md:739-743,761-773`); there is nothing to mirror. coco-rs's
  dream pass already covers model-driven consolidation.
