# jcode vs coco-rs — Adversarial Architecture Review & Optimization Roadmap

> Synthesis of an 11-module, source-level comparison of two independent Rust
> coding-agent harnesses: **jcode** (`/lyz/codespace/3rd/jcode`, ~457K LoC, an
> independent performance-obsessed lineage) and **coco-rs**
> (`/lyz/codespace/codex/coco-rs`, ~609K LoC, a behavior-faithful Rust port of
> Anthropic's Claude Code). Every per-module finding below was read at source on
> both sides and run through an adversarial verifier; the per-module files
> (`01`–`11`) carry the load-bearing `file:line` citations. This overview does
> not re-derive them — it ranks, de-conflicts, and decides what coco-rs should
> and should not learn.

---

## Executive summary — what coco-rs should learn from jcode, and what it should NOT copy

The single most important framing: **jcode and coco-rs sit at opposite ends of an
architectural spectrum, and most of the headline differences trace back to one
fork** — jcode is a *persistent multi-session server* with a thin TUI client and
a monolithic root crate; coco-rs is a *single standalone process per session*
with a fine-grained layered crate graph that faithfully mirrors Claude Code's
observable behavior. A difference between them is therefore rarely a coco-rs
defect; it is usually a consequence of that fork plus coco-rs's documented
non-goals.

**What coco-rs should genuinely learn (the high-value, non-goal-respecting wins):**

1. **Embedding-assisted memory recall and write-time dedup** (M06). coco-rs
   already ships a *superior* semantic engine (`coco-retrieval`: multi-model
   embeddings, sqlite-vec/LanceDB, BM25, RRF fusion, reranker, PageRank repo-map)
   but it is **not wired into the memory crate** — recall ranks one-line
   *descriptions* via an LLM, so a memory whose body is relevant but whose
   description doesn't match is invisible, and there is no deterministic
   write-time dedup between 24h dream passes. Wiring the existing embedder behind
   the existing `Feature::Retrieval` gate (body pre-filter + ≥0.9 dedup) is the
   single highest-leverage *capability* improvement, and it conflicts with no
   non-goal.

2. **Structure-aware code navigation as a model-callable tool** (M08). jcode's
   agentgrep returns a per-file symbol skeleton ("function X @ lines 40–70") in
   one call, letting the model locate code without a follow-up read; its `find`
   ranks files by relevance. coco-rs ships an even deeper engine
   (`RetrievalFacade::search` / `generate_repomap`, tree-sitter tags) but
   `grep -rn RetrievalFacade core/tools app/query` returns **zero hits** — the
   model cannot reach it. Exposing it as a gated `RetrievalTool` plus an opt-in
   symbol skeleton on GrepTool closes a real navigation-UX gap with infrastructure
   coco-rs already owns.

3. **Background (non-blocking) full compaction + a smarter proactive trigger**
   (M07). coco-rs awaits full LLM compaction *inline* on the threshold-crossing
   turn; jcode spawns it and swaps the summary in later. A deferred-swap-in
   handle plus an EWMA token-growth trigger (reframed as a *trigger* for the
   existing full strategy, **not** a fourth strategy) hides latency without
   touching the documented micro/full/reactive taxonomy.

4. **The Claude Code OAuth wire contract + a token-refresh executor** (M09). This
   is the most consequential *functional* gap: a Max/Pro subscriber holding only
   a Claude.ai OAuth token cannot authenticate for inference in coco-rs — it
   models `OAuthTokens::needs_refresh()` but has **no refresh executor** and emits
   only the baseline `claude-code-20250219` beta. The *wire contract* (tool-name
   remap, identity prepend, `oauth-2025-04-20`, `claude-cli` UA) unlocks an
   already-supported credential type; only token *minting/login UI* stays out of
   scope per the dropped `services/oauth/` non-goal.

5. **Cross-agent file-shift conflict notifications** (M05). For concurrent
   multi-agent-in-one-repo work, jcode's "agent B just edited a file under agent
   A's feet, with overlapping-vs-same-file line granularity and the editor's
   intent" is the single most useful swarm primitive, and coco-rs has *nothing*.
   It is portable to coco-rs's team model if scoped correctly (a team-shared
   file-activity log for cross-process panes, structured `FileConflict`
   `ProtocolMessage`).

6. **Cheap, low-risk operational hygiene that costs almost nothing** (M01, M10):
   cap glibc malloc arenas (`mallopt(M_ARENA_MAX,4)`), add startup-phase
   instrumentation so TTFF is *measurable*, raise `RLIMIT_NOFILE`, set a process
   title, and **port jcode's ratcheting CI budget family** (file-size, test-size,
   panic, swallowed-error) plus a touched-file compile-benchmark harness — wired
   into `just quick-check`/hooks, since coco-rs has **no live quality CI**.

**What coco-rs should NOT copy (and why):**

- **The persistent shared server / thin client itself** (M01, M04). It is the
  root cause of jcode's 14ms TTFF and ~10MB/session — but it *is* coco-rs's
  separate, larger deferred **concurrent-app-server** initiative
  (`docs/coco-rs/concurrent-app-server-plan.md`), not a drop-in. Do not frame any
  startup optimization as "mimic jcode's thin-client paint."
- **Self-dev (the agent rebuilding/reloading its own binary)** (M11). Out of
  scope, not merely unimplemented — it is fundamentally incompatible with
  coco-rs's faithful-port identity, single-fixed-binary posture, and no-`unsafe`
  rule. Only the *defensive primitive* (validate-before-activate + rollback) is
  portable, to plugin hot-reload / MCP reconnect.
- **Semantic/embedding compaction triggers and provider-string cache accounting**
  (M07). Collides with the "3 generic strategies, no embedding-in-compaction"
  posture and the "provider concerns live in `vercel-ai-*`" layering rule.
- **A push scheduler for swarm tasks** (M05). coco-rs's pull/self-claim model is
  TS-faithful and deliberate; only an *advisory* dependency-affinity claim-ordering
  hint is in-scope, never the central 5-key push scheduler.
- **Provider-side plan/rate-window probes** (M09). Directly re-creates the
  intentionally dropped `services/claudeAiLimits.ts` / `services/policyLimits/` /
  `services/rateLimitMessages.ts`; acceptable only as a strictly read-only
  `/usage` diagnostic over data coco-rs already holds.
- **A persisted per-memory embedding column / typed-edge memory graph** (M06).
  Would make `coco-memory` own a vector store and break the "human-readable,
  model-curated files" invariant; cache embeddings in `coco-retrieval` instead.

**Where coco-rs is already ahead** (detailed below): native-scrollback TUI with
zero retained-history RAM, a single unified turn loop vs jcode's triplicated
~3,400-LoC loop, typed `StopReason` with no string-sniffing, bounded backpressure
everywhere vs jcode's unbounded mpsc, typed `ProtocolMessage` envelopes vs jcode's
free-text alerts, an already-shipped layered crate graph (jcode's "target
architecture" RFC is roughly coco-rs's *current* layout), a cache-break detector
jcode has no equivalent of, and far stronger memory path-hardening + secret
redaction.

---

## Methodology & adversarial process

Each of the 11 modules was produced by a two-role adversarial pipeline, then
synthesized here:

1. **Per-module analyst** read both codebases for one concern (startup, TUI
   rendering, UI features, multi-session server, swarm, memory, turn lifecycle,
   tools, providers, build/decomposition, ambient/self-dev) and proposed
   optimizations with `file:line` evidence on both sides.
2. **Independent skeptical verifier** re-read the cited source — *not* the
   analyst's prose — and assigned each suggestion a verdict, often surfacing
   "missed findings" the analyst omitted.
3. **This synthesis** ranks the surviving suggestions across modules, de-conflicts
   them against coco-rs's documented non-goals, and drops or re-scopes anything
   that fails source verification.

**What the verdicts mean:**

- **Confirmed** — the jcode mechanism *and* the coco-rs gap were verified at
  source exactly as the analyst framed them; the recommendation stands as written.
- **Nuanced** — the underlying gap is real, but the analyst's *framing,
  mechanism, or scope* was wrong in a way that materially changes the
  recommendation. The correction is folded into the recommendation (e.g. M01-S2:
  jcode's 14ms is a client/server artifact, not an in-process paint reorder — the
  valid in-process recommendation is narrower; M02-R1: not a per-frame win but a
  resize/toggle-replay win; M07-S1: the win is avoiding cancel-and-restart, not
  "avoiding full-context re-send").
- **Refuted** — the claim does not hold at source, or adopting it conflicts with a
  documented non-goal. Listed in "Disputed / refuted" below with the source proof.

**Limitations.** (1) Some jcode engines are *external git crates* not vendored in
the checkout — `agentgrep` (M08) and `mermaid-rs-renderer` (M03) were probed from
clones, tagged `[engine]`; their internal magnitudes ("1800× faster") are not
auditable from this repo. (2) jcode's README perf numbers are **single-machine,
10-run PTY measurements** (README confirms "Measured on this Linux machine across
10 interactive PTY launches") — not independently reproduced here; we adjudicated
*what they measure* and *whether the mechanism is real*, not the exact
multipliers. (3) coco-rs's multi-session server is a **1053-line design doc, not
code on disk** — M04 recommendations are gated on that plan shipping. (4) Effort
ratings are relative engineering estimates, not measured.

---

## Verdict on jcode's headline performance claims

| Claim | Source-substantiated? | Transferable to coco-rs? |
|---|---|---|
| **14ms time-to-first-frame** (README: 14.0ms, range 10.1–19.3ms) | **Partially / conditionally.** The number is real but is the *thin-client* number — a separate persistent `jcode serve` process (`dispatch.rs:585`) holds the warm agent state; the client (`tui_launch.rs:117-176`) just paints a loading shell. A **cold first launch must `spawn_server` and is NOT 14ms.** The benchmark is thin-client-vs-cold-monolith, which flatters jcode. | **Not as-is.** The warm-server win = the deferred concurrent-app-server (out of scope as a quick fix). The *in-process* portion (paint a loading shell first, defer the synchronous `discover_memory_files` disk walk + skills/plugins/hooks/theme loads behind first paint) is portable today (M01-S2, high effort / medium-high risk). |
| **245× faster than Claude Code** (README: 3436.9ms baseline) | **Yes as a startup PTY metric, but misleading as a general claim.** It is a process-startup time-to-first-frame benchmark of thin-client vs the TypeScript Claude Code — *not* a render-throughput or per-turn comparison. Nothing in the turn loop or render path substantiates a steady-state 245× advantage. | **N/A.** It compares against TS Claude Code, not coco-rs. The mechanism (warm shared server) is the deferred server work. |
| **48.7ms time-to-first-input** (range 30.3–62.7ms) | **Same basis as TTFF** — thin client against warm server; conditional on a running daemon. | Same as TTFF: in-process reorder is portable; warm-server is the deferred server. |
| **~10MB extra RAM per added session** (README: ~9.9MB embedding-off / ~10.4MB) | **Yes, directionally, and mechanically credible.** An added session is an `Arc<Mutex<Agent>>` + registry entries + a per-session sender in **one shared process** (`server.rs:394`) with a shared MCP pool (`:429-430`) — not a new OS process. | **Only via the deferred concurrent-app-server.** coco-rs's *target* (plan §6: process-wide `AuthManager`/`ToolRegistry`/`CommandRegistry` + per-thread engine/MCP) is architecturally the *same* design; if executed it converges on this profile. Not addressable without that work. |
| **27.8MB idle RAM** (README explicitly "embedding off") | **Honest but stripped.** The 27.8MB is the *embeddings-off* baseline; embeddings-**on** (the default build `["pdf","embeddings"]`) is **167.1MB / 6.0×** (README:71-78). The memory module is jcode's largest RAM line item, footnoted honestly but not in the headline. | **Partly.** Two contributing mechanisms transfer: glibc arena cap `mallopt(M_ARENA_MAX,4)` (M01-S1, portable today). The idle-embedding `malloc_trim` unload (`embedding.rs`) has **no payload in coco-rs** — it ships no resident local embedding model in the default build. |
| **1000+ fps TUI rendering** | **No — marketing.** True only as single-frame *capability* (a `render_frame` is sub-millisecond). The actual loop is event-driven; the default `redraw_fps` is **60** (`jcode-config-types:572-573`), clamped 1–120, idling at ~1fps (250ms–5s intervals) with a 12.5fps spinner. Not a sustained 1000fps. | **Moot.** coco-rs is *also* event-driven with a 120fps *cap* (not a clock), coalesced events, infinite idle sleep, and a ~20fps in-turn spinner. coco-rs already avoids the waste; it has no free-running clock to "fix." |
| **Custom mermaid renderer "1800× faster", no browser/TS dep** | **Mechanism yes, multiplier external, AND not shipped by default.** The browser-free parse→SVG→PNG→terminal pipeline is real (`jcode-tui-mermaid`), but it is behind a non-default Cargo feature (`renderer`, `default=[]`) **never activated by any workspace member**; the shipped `jcode` default is `["pdf","embeddings"]`. So the default jcode build renders mermaid as *text*, same as coco-rs. "1800×" lives in the external renderer repo (cold-puppeteer vs warm in-process). | **As an opt-in scope expansion only** (M03-R2): new Standalone-layer crate, new `Feature::Diagrams` default OFF, rendered into a dedicated overlay (mandatory — native scrollback is line-text), product buy-in required for the heavy resvg/usvg deps. It is a scope expansion beyond *both* harnesses' defaults, not parity. |
| **`restart_snapshot` faster process resume** | **No — it is session *continuity*, not heap snapshot.** `restart_snapshot.rs:184-203` re-launches sessions in new terminals; it does not serialize/restore heap state, so it gives no faster-than-cold process resume. | Not a perf mechanism; nothing to transfer. |
| **40+ providers with multi-account OAuth** | **Breadth yes, depth overstated.** 32 OpenAI-compatible profiles are mostly the *same* generic adapter with different `api_base` constants; the "jcode subscription" router pins to `https://subscription.jcode.invalid/v1` (a stub). Multi-account + Claude.ai-OAuth inference + cross-account failover *are* real and deep (M09). | **Selectively** (M09): the OAuth wire contract + refresh executor (high value), a data-only preset catalog, and round-robin multi-account. **Not** the Bedrock/Vertex/Foundry routes (explicit non-goals) or the subscription stub. |

---

## Top optimization recommendations for coco-rs (ranked, high-impact first)

Ranking weighs **impact × non-goal-fit ÷ effort**, with capability gaps and
cheap-high-confidence wins promoted. "Respects non-goals?" is the gate — anything
**No** is excluded from the actionable list (see Disputed/refuted). IDs map to the
per-module files.

| Rank | Recommendation | Module | Impact | Effort | Respects non-goals? |
|---:|---|:---:|:---:|:---:|:---:|
| 1 | **Expose retrieval RepoMap / hybrid search as a model-callable `RetrievalTool`** (engine already exists, model can't reach it) | M08-S3 | High | Medium | Yes |
| 2 | **Rank memory recall over BODIES via an embedding pre-filter** (descriptions-only recall misses body-relevant memories) | M06-S2 | High | Medium | Yes |
| 3 | **Deterministic write-time memory dedup via the existing embedder** (≥0.9 cosine reject/merge; today only the 24h dream dedups) | M06-S1 | High | Medium | Yes |
| 4 | **Opt-in per-file symbol skeleton on GrepTool content output** (structure-aware grep; reuse tree-sitter tags + jcode's dense-skip CPU guards) | M08-S1 | High | Medium | Yes |
| 5 | **Implement the Claude Code OAuth wire contract** so Claude.ai-subscription tokens drive inference | M09-S1 | High | High | Yes (wire contract; minting/login still out) |
| 5a | **Add an OAuth token-refresh executor** (hard prerequisite for #5; coco-rs models `needs_refresh` but has no executor) | M09-VF-REFRESH | High | High | Yes (refresh-only) |
| 6 | **Background (non-blocking) full LLM compaction with deferred swap-in** (today awaited inline on the threshold turn) | M07-S2 | High | High | Yes |
| 7 | **Cross-agent file-shift (edit-under-feet) conflict notifications** (team-shared activity log + structured `FileConflict`) | M05-S1 | High | High | Yes |
| 8 | **Wire the agent-side Hub connector** (CoreEvent → `coco-hub-protocol` Batch) for live multi-session observability | M04-S6 | High | High | Yes (bridge only `CoreEvent`, keep read-only default) |
| 9 | **Cap glibc malloc arenas at startup** (`mallopt(M_ARENA_MAX,4)` via a `COCO_*` EnvKey, Linux-only, before runtime) | M01-S1 | Medium | Low | Yes |
| 10 | **Add startup-phase instrumentation** (coco_otel startup-profile) — DO FIRST; prerequisite to measure #9/#11/#16 | M01-S3 | Medium | Low | Yes |
| 11 | **Proactive EWMA token-growth compaction *trigger*** (not a 4th strategy; port jcode's field defaults; default OFF) | M07-S3 | Medium | Medium | Yes |
| 12 | **Make inline tool-output truncation budget-aware** (live remaining-context, keep persist-to-disk for full text) | M08-S4 | Medium | Medium | Yes |
| 13 | **Port jcode's ratcheting CI budget family** (file-size + test-size + panic + swallowed-error) into `just quick-check`/hooks | M10-S1, M10-S7 | Medium | Low-Med | Yes |
| 14 | **Text-wrapped tool-call recovery for OpenAI-compatible providers** (`to=functions.NAME{...}` salvage; capability-gated) | M07-S5 | Medium | Medium | Yes |
| 15 | **Content-mining breadcrumb on reactive head-drop** (model is currently blind to peeled content; reuse `extract_discovered_tool_names`) | M07-S4 | Medium | Low | Yes |
| 16 | **Host-aware TUI performance tier** (degrade in-turn fps/spinner on SSH/WSL; via `DisplaySettings` + `COCO_*`, no ad-hoc env) | M01-S4 | Medium | Medium | Yes |
| 17 | **Per-teammate liveness heartbeat + stale detection** (independent timer, `RunningStale` status; not tool-boundary) | M05-S3 | Medium | Medium | Yes |
| 18 | **Task reassign/salvage verbs** that move a task to a different worker with preserved progress | M05-S4 | Medium | Medium | Yes |
| 19 | **Topic-change detection to trigger mid-session memory extraction** (embedding cosine; lexical fallback when off) | M06-S4 | Medium | Medium | Yes |
| 20 | **Continuation on the full abnormal-stop_reason family** (audit OpenAI-compat finish-reason→`MaxTokens` mapping at the seam) | M07-S7 | Medium | Low-Med | Yes (reinforces the seam non-goal) |
| 21 | **Reconnect-ownership / session-takeover state machine** for multi-client attach (`AttachDecision`; default `RejectConflict`) | M04-S3 | Medium | High | Yes |
| 22 | **Match-centered long-line compaction for Grep content lines** (today hard-cuts at byte 500, dropping far matches) | M08-S5 | Medium | Low-Med | Yes |
| 23 | **Bounded overnight/long-run supervisor** with structured task cards + morning report (bounded, user-initiated; no daemon) | M11-S3 | Medium | High | Yes |
| 24 | **Per-session recall repeat/overlap suppression** on the injection path (today a surfaced path is *permanently* skipped until `/clear`) | M06-S5 | Medium | Low | Yes |
| 25 | **Write-time deterministic contradiction handling** (bias extraction fork toward Edit on high-similarity same-type memory) | M06-S7 | Medium | Medium | Yes |
| 26 | **Periodic single-topic extraction safety net** (turn-count trigger so long same-topic sessions still extract) | M06-S8 | Medium | Low | Yes |
| 27 | **Multi-account support with round-robin rotation** for API-key/token providers (sequence after #14's classifier) | M09-S4 | Medium | High | Yes (round-robin; headroom-ranked needs the refuted R5) |
| 28 | **Broaden the fallback trigger to a typed classifier** (add 402/billing arm + digit-boundary matcher; fixes a `529`-substring false-positive) | M09-S2 | Medium | Medium | Yes (drop "sideline credential" until multi-account) |
| 29 | **Memoize per-cell wrapped lines** to make resize/toggle replay incremental (NOT per-frame; native scrollback already O(0) on scroll) | M02-R1 | Medium | Medium | Yes |
| 30 | **Avoid O(messages×cells) re-render in the replay overflow-trim walk** (prefix-sum + binary search; only bites >9000-row histories) | M02-R2 | Medium | Low | Yes |
| 31 | **Virtualize the compatibility-fallback (Viewport-mode) live tail** (Zellij etc. re-wrap the whole transcript every frame) | M02-MF-R6 | Medium | Medium | Yes |
| 32 | **Inter-agent topic channels (pub-sub)** + wire the dead `subscriptions` field | M05-S7 | Low-Med | Medium | Yes |
| 33 | **Add `checkpoint_summary`/`last_detail` to `TaskProgress`** for higher-fidelity salvage handoff | M05-S8 | Low-Med | Low | Yes |
| 34 | **Surface a path-derived per-file role hint + standalone outline mode** (fold into #1/#4) | M08-S6 | Low-Med | Low | Yes |
| 35 | **Structured conversation-history retrieval tool** (deterministic handle over past transcript; BM25-backed optional) | M06-S9 | Low-Med | Medium | Yes |
| 36 | **Take recall off the turn's serial critical path** (spawn concurrently with prompt build + join; optional N+1 prefetch behind a flag) | M06-S3 | Medium | Medium | Yes |
| 37 | **Streaming-only mid-turn steering injection** (no-cancel soft interrupt at the post-batch streaming point only; new Feature) | M07-S1 | High | High | Yes (streaming-only; honors the pairing non-goal) |
| 38 | **Deliver urgent teammate messages at the earliest safe turn boundary** (cancel `current_turn_cancel`, NOT reuse CommandQueue) | M05-S2 | High | High | Yes (turn-cancel approach; not mid-turn injection) |
| 39 | **Upgrade skill_discovery ranking** lexical→semantic (Memory/Fast LLM query for TS-parity, or embedding behind `Feature::Retrieval`) | M11-S2 | Medium | Medium | Yes |
| 40 | **Pre-activation validate + rollback for live swaps** (plugin hot-reload primary, MCP reconnect secondary; NOT self-dev) | M11-S5 | Low | Medium | Yes |
| 41 | **Touched-file compile-benchmark harness** (port `bench_compile.sh --touch/--runs/--json`; report sccache hit/miss) | M10-S3 | Medium | Low | Yes |
| 42 | **Cross-layer dependency-direction guard** in `just quick-check` (reuse in-repo `verify_tui_core_boundary.py`; rank `coordinator`/`hub`/`standalone`) | M10-S2 | Medium | Low | Yes |
| 43 | **Proactively drop truncation-mangled (null-input) tool calls** before dispatch (scope to the `MaxTokens` branch) | M07-S8 | Low-Med | Low | Yes |
| 44 | **Reconcile mold-vs-sccache doc drift** in root CLAUDE.md (sccache is primary cache; document why `incremental=false`) | M10-S6 | Low | Low | Yes |
| 45 | **Bounded team event journal** for coordinator visibility + restart catch-up (isolated from CoreEvent) | M05-S6 | Medium | Medium | Yes |
| 46 | **Terminal-result-required continuation** for forked/background agents (exactly one nudge, reuse `BudgetTracker`) | M11-S4 | Low | Low | Yes |
| 47 | **Automatic RGB→xterm-256 downsampling** for non-truecolor terminals (apply once at the `UiStyles` facade) | M02-R4 | Low | Low | Yes |
| 48 | **Raise `RLIMIT_NOFILE`** soft limit at boot (best-effort, for many-MCP/many-session workloads) | M01-S6 | Low | Low | Yes |
| 49 | **Set a stable process title / `PR_SET_NAME`** so `ps`/`kill`/multi-session tooling can identify sessions | M01-S5 | Low | Low | Yes |
| 50 | **Fine-grained recall telemetry** (`MemoryEvent::RecallRanked`, metadata-only) — prerequisite for tuning #2/#24 | M06-S6 | Low | Low | Yes |
| 51 | **Gate the always-on retrieval chunking deps** (tree-sitter grammars + tiktoken + rusqlite are unconditional in coco-retrieval) | M10-S4 | Low | Low | Yes |
| 52 | **Split >2500-LoC files** for readability/conflict-surface (NOT compile speed — `incremental=false` recompiles whole crate front-end) | M10-S5 | Low | Medium | Yes |
| 53 | **Opt-in inline-diagram (Mermaid) renderer** in a Standalone crate behind `Feature::Diagrams` default-OFF, overlay-rendered | M03-R2 | Medium | High | Yes (explicit opt-in scope expansion) |
| 54 | **Persistent agent-writable scratch/notes panel as a tool** (overlay-rendered, route persistence via coco-session) | M03-S4 | Low | High | Yes (explicit opt-in scope expansion) |
| 55 | **Per-tool background detach + reload-resumable results** (extend existing `signal_detach`/`AsyncLaunched`) | M07-S6 | Low | High | Yes — **DEFER** (only valuable in serve/connect mode) |

> **Sequencing notes.** Do **M01-S3 (startup instrumentation, #10) first** — it
> makes #9/#16 measurable. The **M06 embedding work (#2/#3/#19/#25)** shares one
> seam (inject `EmbeddingProvider` behind `Feature::Retrieval`) and should be
> planned as one effort, not five patches. **M09-S1 depends on M09-VF-REFRESH**;
> **M09-S4's headroom-ranked variant depends on the refuted R5** — ship round-robin
> only. **M02-R2/MF-R6 stack on M02-R1**. The CI/build items (#13/#41/#42/#44) must
> target `just`/hooks, **not** a GitHub workflow — coco-rs has none for quality.

---

## Cross-cutting themes

Six themes recur across the 11 modules and explain most of the divergence:

1. **Performance-as-a-feature vs behavior-fidelity-as-a-feature.** jcode
   *productizes* latency and RAM — `startup_profile` marks across the whole boot
   path, `process_memory` smaps/PSS sampling, a startup-time CI ratchet, a perf
   tier that degrades on SSH/WSL, `mallopt`/jemalloc tuning, an idle-embedding
   unload. coco-rs optimizes *correctness and observable parity* — typed
   `StopReason`, single-writer transcript invariant, cache-break detection, the
   3-tier error policy. The transferable slice is the *measurement* discipline
   (instrumentation, ratchets, the arena cap) — not the productized claims.

2. **Fine-grained crate decomposition (coco-rs already won this).** jcode is a
   "modular monolith" (its own RFC's words): ~87% of non-desktop code is in one
   336K-LoC root crate that can't use sccache and OOMs `rustc`. coco-rs's largest
   crate is `app/tui` at ~9%, with enforced layering, a dual-seam isolating the
   *entire* Vercel AI SDK behind 2 crates, and crate-boundary sccache. **jcode's
   "target architecture" RFC is roughly coco-rs's current layout.** What coco-rs
   lacks is jcode's *ratcheting CI budgets* and *compile-benchmark harness* —
   tooling, not architecture.

3. **Server-first multi-session (the one big structural fork).** jcode ships a
   hardened single-process/multi-client peer server (ready-fd handshake, flock,
   in-place exec hot-reload, session takeover, idle/owner-pid lifecycle). coco-rs
   has a read-only observability hub, a single-session SDK server, and a 1053-line
   plan for the true equivalent — with *better* starting boundaries (bounded
   channels everywhere vs jcode's unbounded mpsc; two-level `ThreadId`/`SessionId`
   identity vs jcode's bare `session_id: String`; layered `app/server`→`app/thread`
   vs jcode's 29-arg `handle_client` god-struct). The capability gap is a
   **maturity gap, not an architecture refusal**; all M04 recommendations gate on
   the plan shipping.

4. **Passive semantic memory vs file memory.** jcode built genuine passive recall
   (embed-and-verify, off-thread, one-turn-behind) with a memory graph and ambient
   consolidation. coco-rs faithfully ports Claude Code's *file* memory (LLM ranker
   over descriptions). The irony: coco-rs owns a *deeper* semantic engine
   (`coco-retrieval`) than jcode's single MiniLM — it just isn't wired into memory
   *or* the tool path. The highest-value M06/M08 recommendations are **seam
   tasks** (wire the existing engine in behind `Feature::Retrieval`), not
   build-an-engine tasks. coco-rs is also clearly ahead on memory *security*
   (path-traversal/symlink hardening + executed secret scanner + forked-subagent
   write fence vs jcode's doc-only "don't remember secrets").

5. **UI as negative space vs UI as a single-column transcript.** jcode packs
   status widgets into measured empty margins (a 15-kind `WidgetKind` system + a
   2-D bin-packer with anti-flicker hysteresis), ships a persistent agent-writable
   side panel as a tool, and renders inline images/mermaid. coco-rs deliberately
   keeps a single-column transcript on **native terminal scrollback** (its
   documented target) — *zero retained-history RAM*, terminal-native
   scroll/select/copy for free, a pure-derivation transcript with a single-writer
   invariant. Three of jcode's four UI features are outside coco-rs's
   Claude-Code-parity scope by design; coco-rs's rendering layer is
   architecturally *cleaner* than jcode's self-acknowledged "state hub." The
   negative-space "get out of the way" claim is also situational — jcode's widgets
   collapse below `MIN_WIDGET_WIDTH=24` on narrow terminals, while coco-rs's inline
   rows (already 0-height when idle) keep showing status.

6. **Mid-turn steering: expressiveness vs provider-safety.** jcode's soft-interrupt
   (3 API-safe inject points, urgent-abort with stub `tool_results`) is its
   headline turn-loop differentiator. coco-rs *deliberately deleted* its mid-turn
   `Now`-drain to preserve `tool_use`/`tool_result` pairing on non-streaming
   providers. The verified win is narrow — avoiding the *hard cancel-and-restart*,
   not "avoiding full-context re-send" (both re-send full history every
   continuation) — and the only non-goal-respecting paths are streaming-only
   post-batch injection (M07-S1) and a turn-boundary cancel for urgent teammate
   messages (M05-S2), never reusing the CommandQueue mid-turn.

---

## Where coco-rs is already ahead / where jcode's claims do not hold (fairness)

**coco-rs is already equal or better here:**

- **Native scrollback TUI = zero retained-history RAM + terminal-native UX**
  (M02). Finalized cells emit once into the host scrollback; scrolling never
  re-renders. jcode keeps the *whole wrapped transcript resident* and had to build
  a custom terminal ("handterm") to recover smooth scroll. coco-rs also ships its
  own cell-diff + BSU/ESU synchronized-update framing, addressing tearing jcode's
  own docs warn about.
- **Single unified turn loop** (M07) vs jcode's three near-identical ~1000–1300
  LoC loop bodies (broadcast/mpsc injection code is line-for-line duplicated).
- **Typed `StopReason`** set once at the adapter seam, no string-sniffing
  anywhere (M07/M09) — robust across providers and locales vs jcode's ~12-phrase
  substring matching. (Caveat: the same typing is the root of M07-S7 — the seam
  must map every wire reason correctly.)
- **Bounded backpressure end-to-end** (M04): the plan bounds every hop with a
  `-32001` overload rejection; jcode uses *unbounded* mpsc for per-client/session
  delivery — a memory-growth risk under a slow client.
- **Typed `ProtocolMessage` envelopes** (M05): a closed 13-variant tagged union
  vs jcode's free-text + `Other(String)` status enums; plus fine-grained
  permission propagation, a 2-stage LLM handoff safety classifier, and a
  team-memory secret guard jcode has no analog for.
- **An already-shipped layered crate graph** (M10) that jcode's RFC is still
  *planning*; broader seam enforcement (entire SDK behind 2 crates) that is
  *actually run* in `quick-check`/hooks (jcode's boundary guard is advisory, not
  in CI); heavy ML dep default-**off** (jcode ships it default-**on**); 1028
  `.test.rs` companion files keeping prod compile units lean.
- **Typed prompt-cache-break detection** (M09) jcode has *no equivalent* of —
  attributes drops to TTL vs client-change, emits `coco_cache_break_total`, guards
  cache-shared forks; plus deterministic `BTreeSet`-sorted, capability-gated beta
  resolution.
- **Cache-aware multi-provider compaction** (M07): the reactive path queues
  Anthropic's server-side `context_management` (cache-preserving, no local
  mutation) and a provider-agnostic `services/compact` crate; jcode always mutates
  locally and bakes provider strings into the manager.
- **Memory security + a complete MCP OAuth flow** (M06/M09): coco-rs hardens
  null/UNC/drive-root/tilde/URL-encoded paths + symlink-escape walk + a two-ring
  write fence and runs an executed secret scanner; it ships full PKCE+keyring+refresh
  for *MCP* OAuth — proving the LLM-OAuth gap is a deliberate scope line, not an
  inability.
- **More robust auto-dream** (M11): RAII dual-lock (PID+mtime CAS *and*
  process-local atomic) with rollback on cancel vs jcode's fire-and-forget
  embedding backfill and single PID file.

**jcode README claims that do not hold as stated:**

- "1000+ fps" → real default cap is 60fps; loop is event-gated and idles at ~1fps.
- "14ms / 245×" → conditional on a warm server already running; cold launch pays
  `spawn_server`; it is a thin-client-vs-cold-monolith startup PTY benchmark.
- "27.8MB" → the *embeddings-off* baseline; default build is 167.1MB / 6.0×.
- "1800× mermaid" → external multiplier; **and the renderer is off in the default
  build** (behind a never-activated Cargo feature).
- "50 workspace crates" → 50 members exist, but excluding 66K-LoC `jcode-desktop`,
  only ~48K LoC is in crates vs 336K in the root monolith.
- "blazing-fast build" → jcode's own README: ~1 minute now, "goal is 5–20s" —
  explicitly unmet.
- petgraph memory graph / HDBSCAN clustering / "Ambient Garden" / negative+procedural
  memories → unchecked TODOs in jcode's own docs (actual graph is HashMap-based).
- `restart_snapshot` → session re-launch continuity, not heap snapshot/restore.

---

## Disputed / refuted claims from the adversarial review

These were **refuted at source** or excluded because adopting them conflicts with a
documented coco-rs non-goal. (Module-level "nuanced" corrections are folded into
the recommendations above and not repeated here.)

- **M11-S1 — Usage-forecast-aware interval scheduler (REFUTED, dead code).**
  jcode's forecast scheduler is *dead in the running binary*: `usage_log.record()`
  is never called outside `#[cfg(test)]`, both live `calculate_interval` calls pass
  literal `None`, and `RateLimitInfo` is only constructed in tests. With `None` +
  empty log, it degenerates to a **fixed 120-min interval + ×2..×64 backoff** —
  exactly the fixed-cron behavior the claim said it transcends. Porting unused
  scaffolding has no value (and rate-limit data lives in `vercel-ai-*` per a
  non-goal). Only the failure-backoff state machine is live.

- **M09-S5 — Surface provider-side plan/rate-window usage (REFUTED, non-goal).**
  Directly re-creates the intentionally dropped `services/claudeAiLimits.ts` /
  `services/policyLimits/` / `services/rateLimitMessages.ts`; the Anthropic 5h/7d
  OAuth-window probe *is* the Claude.ai-limits surface the port excludes. Has no
  consumer until M09-S1 + M09-S4 land. **DEFER**: acceptable only as a strictly
  read-only `/usage` diagnostic over data coco-rs already holds (429 `reset_at_ms`)
  that never alters request shaping or emits rate-limit policy prose.

- **jcode "subscription router product" (REFUTED, stub).**
  `subscription_catalog.rs:8` pins `DEFAULT_JCODE_API_BASE` to
  `https://subscription.jcode.invalid/v1`; the curated list pins to a `Stealth`
  stub "until a cache-capable route exists." A placeholder, not a working billing
  backend — nothing to mirror.

- **jcode Anthropic cloud routes — Bedrock/Vertex/Foundry (OUT OF SCOPE).**
  Explicit documented non-goals for coco-rs. coco-rs keeps the `AuthMethod` arms
  for env detection/diagnostics only; `model_factory` never dispatches on them.
  jcode's breadth here is irrelevant to a fair comparison.

- **Self-dev (agent rebuilds/reloads its own binary) (OUT OF SCOPE).**
  Fundamentally incompatible with coco-rs's faithful-port identity, single-fixed-binary
  posture, and no-`unsafe` rule. Only the *defensive primitive*
  (validate-before-activate + rollback) is portable, re-homed to plugin hot-reload
  / MCP reconnect (M11-S5).

- **Push scheduler for swarm tasks (M05-S5 wholesale, REFUTED for scope).**
  The 5-key dependency/load-affinity *push* sort conflicts with coco-rs's
  deliberate pull/self-claim model (TS-faithful). Only an *advisory*
  `preferred_owner` claim-ordering tiebreaker survives; load-sort needs a central
  plan-DAG view coco-rs lacks.

- **Mid-turn `CommandQueue` reuse (M05-S2 / M07-S1 original framing, REFUTED).**
  coco-rs *deliberately deleted* its mid-turn `Now`-drain to preserve
  `tool_use`/`tool_result` pairing on non-streaming providers. The teammate path
  wires no CommandQueue at all. Re-scoped to a turn-boundary cancel (M05-S2) and a
  streaming-only post-batch injection (M07-S1).

- **Global content-hash highlight LRU (M03-S1 original framing, REFUTED).**
  Wrong cost model (coco-rs has no syntect — highlighting is a flat char scanner)
  and wrong render model (native scrollback re-highlights only on
  width/viewport/theme change — exactly the events a hash key would invalidate on).
  Narrowed to an *incremental live-cell* renderer for the one streaming cell, gated
  on profiling.

- **Persisted per-memory embedding column / typed-edge memory graph (M06, REFUTED
  for design).** Would make `coco-memory` own a vector store and break the
  "human-readable, model-curated files" invariant. Cache body embeddings in
  `coco-retrieval`'s existing mtime/content-hash cache instead (derived,
  disposable). HDBSCAN/"Ambient Garden" are unimplemented in jcode too — nothing to
  mirror.

- **Semantic/embedding compaction trigger + provider-string cache accounting (M07,
  REFUTED for non-goal).** Collides with the "3 generic strategies, no
  embedding-in-compaction" posture and the "provider concerns in `vercel-ai-*`"
  layering rule. The token-statistical EWMA *trigger* (M07-S3) was salvaged; the
  embedding half is dropped.

---

## Per-module index

| Module | File | One-line takeaway |
|---|---|---|
| **M01 — Startup & Runtime Performance** | [`01-startup-and-runtime-performance.md`](01-startup-and-runtime-performance.md) | jcode's 14ms TTFF is a warm-client/server artifact (not addressable without the deferred server); coco-rs should add startup instrumentation, cap glibc arenas, and reorder first-paint in-process. |
| **M02 — TUI Rendering Architecture** | [`02-tui-rendering-architecture.md`](02-tui-rendering-architecture.md) | Opposite scrollback choices, not better/worse — coco-rs's native scrollback already gives 0-RAM history + free terminal scroll; the only wins are resize/toggle-replay memoization and a fallback-mode virtualization. |
| **M03 — UI Features (Side Panels, Widgets, Mermaid, Images)** | [`03-ui-features-sidepanel-widgets-mermaid.md`](03-ui-features-sidepanel-widgets-mermaid.md) | Three of four jcode UI features are outside coco-rs's parity scope by design (and mermaid isn't even in jcode's default build); coco-rs's transcript-authority model is cleaner than jcode's self-admitted "state hub." |
| **M04 — Multi-Session Server / Client** | [`04-multi-session-server-client.md`](04-multi-session-server-client.md) | A maturity gap, not an architecture refusal — jcode ships the server; coco-rs has a 1053-line plan with *better* boundaries (bounded channels, two-level identity, layered crates). Wire the Hub connector; gate everything else on the plan. |
| **M05 — Swarm / Multi-Agent Coordination** | [`05-swarm-multi-agent-coordination.md`](05-swarm-multi-agent-coordination.md) | jcode wins concurrent-in-one-repo work (file-shift conflicts, mid-turn redirect, salvage); coco-rs wins typed envelopes, permission propagation, and handoff security. Port file-shift + heartbeat + salvage, keep the pull model. |
| **M06 — Memory Architecture** | [`06-memory-architecture.md`](06-memory-architecture.md) | coco-rs owns a *deeper* semantic engine than jcode but doesn't wire it into memory — recall over descriptions misses body-relevant memories. The top wins are seam tasks behind `Feature::Retrieval`; coco-rs is ahead on memory security. |
| **M07 — Agent Turn Lifecycle & Compaction** | [`07-agent-turn-lifecycle-compaction.md`](07-agent-turn-lifecycle-compaction.md) | coco-rs wins maintainability + provider-correctness (one loop, typed reasons, cache-aware reactive); jcode wins latency-hiding (background compaction) and steering UX. Port background compaction + EWMA trigger + text-wrapped recovery, not semantic compaction. |
| **M08 — Tools System (structure-aware grep)** | [`08-tools-system.md`](08-tools-system.md) | coco-rs has a richer typed tool contract and a deeper code-intel stack, but the model can't reach it — expose `RetrievalFacade` as a tool and add an opt-in symbol skeleton to grep. Exposure-aware collapse belongs on a future nav tool, not grep. |
| **M09 — Providers / Inference / OAuth / Multi-Account** | [`09-providers-inference-oauth.md`](09-providers-inference-oauth.md) | The consequential gap is Claude.ai-subscription inference (wire contract + refresh executor); the "40+ providers" breadth is mostly one adapter with different base URLs, and Bedrock/Vertex/Foundry are non-goals. |
| **M10 — Crate Decomposition & Build Performance** | [`10-crate-decomposition-build-perf.md`](10-crate-decomposition-build-perf.md) | coco-rs already shipped the layered decomposition jcode's RFC is planning, and isolates heavy deps better; the gap is jcode's ratcheting CI budget family + compile-benchmark harness — wire into `just`/hooks (coco-rs has no quality CI). |
| **M11 — Ambient / Overnight / Self-Dev / Extensibility** | [`11-ambient-overnight-selfdev-extensibility.md`](11-ambient-overnight-selfdev-extensibility.md) | jcode's forecast scheduler is dead code; self-dev is out of scope (faithful-port + single-binary). Portable: a bounded user-initiated overnight supervisor, semantic skill ranking, and a validate-then-activate guard for plugin/MCP swaps. |
