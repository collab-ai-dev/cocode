# Ambient / Overnight / Self-Dev / Extensibility: jcode vs coco-rs

This module compares the "the agent runs itself" surface across the two
harnesses: always-on ambient autonomy, bounded overnight autonomy, the agent
rebuilding/reloading its own binary (self-dev), and the semantic/extensibility
layer (skill injection, browser, dictation, hooks/plugins). Every claim below
was re-read against source on both sides; file:line refs are load-bearing and
were verified, not copied from either README.

jcode (independent lineage) treats autonomy as a first-class subsystem.
coco-rs deliberately mirrors Claude Code, which ships **no** always-on ambient
agent, **no** overnight coordinator, and **no** self-modification. A capability
difference here is therefore not automatically a coco-rs deficiency — it is
judged on engineering merit for coco-rs's stated goal (faithful Claude Code
port + single fixed binary + multi-provider layering).

---

## jcode approach

Four cooperating engines.

**Ambient mode (always-on).** A persistent server spawns a single tokio loop,
`AmbientRunnerHandle::run_loop` (`src/ambient/runner.rs:532`). Each iteration:
read state → `AdaptiveScheduler::should_pause()` (pauses while a user session is
active, runner.rs:588) → GC stale permission requests
(`expire_dead_session_requests`, runner.rs:603-622) →
`AmbientManager::take_ready_direct_items()` for session/spawn-targeted reminders
(runner.rs:628) → `should_run()` → acquire a **single-instance PID lock**
(`AmbientLock::try_acquire`, runner.rs:695) → `run_cycle`. Sleep is
`min(scheduler interval, time-to-next-direct-item)` (runner.rs:663-678). The
loop is interruptible: a `wake_notify` `Notify` short-circuits the sleep
(runner.rs:685-690, 818-823), and external channels (Telegram/Discord/IMAP) push
into the running agent's soft-interrupt queue via `active_cycle_queue`
(runner.rs:907-911).

`run_cycle` (runner.rs:877-974) forks the provider, registers ambient tools,
builds a context-rich system prompt from memory-graph health + recent sessions +
feedback memories (`build_cycle_context`, runner.rs:837-849), runs the agent
once, and **requires** the agent to call `end_ambient_cycle`. Robust
unexpected-stop handling: if `take_cycle_result()` is empty after the first turn,
it injects exactly one continuation prompt ("You stopped unexpectedly without
calling end_ambient_cycle…", runner.rs:937-942) and re-checks; if still empty it
emits a **forced `CycleStatus::Incomplete`** with the conversation transcript
preserved (`conversation: Some(agent.export_conversation_markdown())`,
runner.rs:960-971) rather than hanging. After a successful cycle it persists a
structured `AmbientTranscript` (session id, started/ended, status,
provider/model, summary, full conversation, pending permissions,
memories_modified, compactions; runner.rs:739-759), dispatches a cycle-summary
notification (runner.rs:762), and fire-and-forgets a memory-embedding backfill
(`MemoryManager::backfill_embeddings`, runner.rs:765-783).

**Adaptive scheduler — see the refutation below.** `AdaptiveScheduler`
(`src/ambient/scheduler.rs:168-277`) is *designed* to keep a rolling 24h
per-source `UsageLog` (`UsageSource::{User, Ambient}`), project user burn over
the rate-limit window, reserve 80% of headroom for the user
(`user_budget_reserve=0.8`), divide the remaining ambient budget by
avg-tokens-per-cycle, clamp to [5,120] min, and apply ×2..×64 backoff. In the
**running binary this forecast is dead** (verified below); only the
failure-backoff state machine is live.

**Overnight mode (bounded, with a morning report).** Crate
`jcode-overnight-core` (pure logic: chrono/serde only) owns the manifest,
`OvernightEvent`, the structured task-card schema, and prompt builders.
`run_supervisor` (`src/overnight.rs:229-380`) is a phase machine driven entirely
by which prompt it injects next: preflight (`gather_preflight` aggregates usage
across **all** providers + resource + git, overnight.rs:243) → coordinator turns
→ at `handoff_ready_at` inject handoff prompt (overnight.rs:283-293) → at
`target_wake_at` inject morning-report prompt (overnight.rs:326-341) → bounded
`post_wake_grace` continuation (overnight.rs:343-353) → final wrap-up
(overnight.rs:355-366) → completed. Cancellation is cooperative — the loop
**re-reads the manifest every turn** (`load_manifest`, overnight.rs:265) and
stops on `CancelRequested`. `run_turn_monitored` (overnight.rs:382-429) uses
`tokio::select!` to sample resources every 5 min and emit a "still running after
Nm" event every 30 min. Artifacts: `events.jsonl` + a continuously re-rendered
`review.html` (overnight.rs:252, 302, 313) + per-task cards with a mandated
schema (`write_task_card_schema`, overnight.rs:970-1018) requiring
why_selected/before/after/validation evidence. It is reachable in the live
binary (TUI `handle_overnight_command`, progress card, cancel wiring).

**Self-dev (the agent rebuilds/reloads its own binary).** Session-local
capability on the shared server (not a separate daemon). The reload pipeline
(`src/tool/selfdev/reload.rs`) is production-grade: compute source state →
publish a versioned build → **`build::smoke_test_server_binary` BEFORE
`build::update_shared_server_symlink`** (reload.rs:289 vs reload.rs:310) →
record a `PendingActivation` with `rollback_pending_activation_for_session` on
**every** failure branch (reload.rs:312, 330, 348, 381, 391) → reload
signal+ack → readiness handshake (`await_reload_handoff`, reload.rs:372,
rollback on Failed/Idle). Crucially, a persisted `ReloadContext` resumes every
affected session after the swap: `continuation_message` returns "Reload
succeeded (vX → vY)… Continue immediately from where you left off. Do not ask
the user what to do next." (reload.rs:101-111), replaying background-task notes.
The smoke-test-before-symlink + rollback + cross-session recovery is what makes
this safe rather than a gimmick.

**Semantic skill injection.** `Skill::as_memory_entry` turns every skill into a
synthetic `MemoryEntry` with `search_text`; `synthetic_skill_entries`
(`src/memory.rs:608-639`) adds them to the retrieval candidate set with
embeddings (`ensure_embedding`). This is **live** (not test-only): the recall
path embeds the query and scores skills by cosine similarity alongside memories,
so an un-invoked skill surfaces by semantic match.

**Browser & dictation.** `src/tool/browser.rs` ships a first-class `browser`
tool: a `BrowserProvider` trait (browser.rs:101) with a static
`FirefoxBridgeProvider` (browser.rs:116-119) speaking native-messaging to a
Firefox extension (~14 actions). `src/dictation.rs` runs a user-configured STT
shell command then `wtype`-injects text into the focused jcode session, resolving
the target by walking niri/`/proc` window→pid (OS-specific but a real hands-free
path).

---

## coco-rs approach

coco-rs ships the Claude-Code-faithful primitives that overlap parts of this
module; it does not ship the always-on or self-modifying engines.

**Background tasks (the autonomy substrate).** `tasks/src/running.rs` —
`TaskManager` keeps two parallel maps: serializable `rows: HashMap<id,
TaskStateBase>` and runtime-only `controls: HashMap<id, TaskControl>`
(`CancellationToken`/`watch::Sender`/`Notify`/`JoinHandle`, never serialized).
Backgrounded agents/shells, in-process teammates, and the Dream task run here;
terminal completions enqueue `<task-notification>` to the main agent. It is
genuinely concurrent and event-emitting (`with_event_sink`, typed
`CoreEvent::Protocol(TaskStarted/Progress/Completed)`) — but **reactive**: a task
runs because the model/user/engine started it, not because a loop woke the agent.
`tasks/src/task_list.rs` holds durable, file-locked plan items; each `Task` has
`id`, `subject`, `status`, **and a `metadata: Option<HashMap<String, Value>>`
blob** (task_list.rs:81) with merge semantics (task_list.rs:96, 519-528).

**Auto-dream (the ambient-consolidation analog — coco-rs is strong here).**
`memory/src/service/dream.rs` — `DreamService` is a per-turn three-gate
scheduler (time ≥ `dream_min_hours`, ≥ `dream_min_sessions`, 10-min scan
throttle) with a PID+mtime CAS file lock (`memory/src/lock.rs`) **and** a
within-process `AtomicBool` (`try_claim_consolidating`, dream.rs:236) to stop a
manual `/dream` racing an auto-dream after the file lock became
same-process-reclaimable. On fire it forks a `ModelRole::Memory` subagent fenced
to the memdir (`allowed_write_roots`, `create_auto_mem_handle`) with
`skip_transcript`; on failure it rolls the lock mtime + scan-throttle stamp back
so the cadence isn't reset (dream.rs:432-446). This closely matches jcode's
"consolidate memory during sleep" idea — but it is invoked by the engine each
turn, not by an independent background loop.

**Scheduling (CRUD + remote triggers — store only).**
`core/tools/src/tools/scheduling.rs` — `CronCreate/CronDelete/CronList` (5-field
validator, MAX_JOBS=50, 7-day auto-expire) and `RemoteTrigger`. All are
`should_defer()` deferred tools delegating to `ctx.schedules: ScheduleStore`.
`core/tool-runtime/src/schedule_store.rs` — `ScheduleStore` is **CRUD-only**
(create/list/run, schedule_store.rs:33-58); the default `NoOpScheduleStore`
errors. There is **no in-process firing loop**: cron/trigger execution is
delegated to a remote backend (CCR). Bundled `/schedule` and `/loop` skills are
gated by `Feature::AgentTriggersRemote`/`AgentTriggers` and route to *remote*
agents. coco-rs's per-(provider, model) `UsageAccumulator`
(`services/inference/src/usage.rs:18-43`, `record` at usage.rs:36) already splits
usage by identity, and generic retry/backoff lives in
`services/inference/src/retry.rs` — but nothing schedules *future* autonomous
work against a usage forecast.

**Extensibility (skills / hooks / plugins / commands — faithful, broad).**
Skills (`skills/`): multi-source discovery, `paths`-glob conditional activation
(`activate_for_paths`), a budgeted listing, **and a proactive
`skill_discovery` producer** (`skills/src/reminder_source.rs:117-170`) — a
*local* case-insensitive substring-on-name + 5+-char word-overlap heuristic over
description/`when_to_use`, capped at 5, feeding the system-reminder pipeline
(`SkillDiscoveryGenerator`, `core/system-reminder/src/generators/audit_add.rs`).
The TS source coco-rs mirrors uses a Haiku-class LLM call
(`services/skillSearch/prefetch.ts`), **not embeddings** (audit_add.rs:11-27).
Hooks (`hooks/`): 27 event types, async registry, SSRF guard, scoped priority.
Plugins (`plugins/`): `PLUGIN.toml` contributions, marketplace, hot-reload.
Commands (`commands/`): v1/v2/v3 slash-command registry.

**Deliberately absent (verified in source).**
- **No self-dev**: grep for `selfdev`/`reexec`/`current_exe`-reexec finds only
  unrelated hits (`utils/cargo-bin`, `exec/sandbox` computing the helper path at
  `exec/sandbox/src/platform/linux.rs:108`, `coordinator/src/spawn.rs:20`). No
  published-build/smoke-test/symlink/reload-recovery pipeline.
- **No native browser tool**: the only non-test browser hit is the
  MCP-deferral prompt text in `core/subagent/src/builtin_prompts.rs:334` telling
  the agent to use `mcp__claude-in-chrome__*`/`mcp__playwright__*` *if present*.
  Browser automation arrives via MCP, not a built-in tool.
- **No dictation/voice feature**: voice exists only as vercel-ai's
  transcription/speech model abstractions (`vercel-ai/ai/src/transcribe/`); the
  `coco-voice` crate does not exist on disk (documented v2 item).
- **No overnight/mission/goal subsystem**: grep for
  `overnight`/`run_supervisor`/`morning_report` returns **zero non-test hits**.

---

## Head-to-head comparison

### 1. Always-on ambient autonomy — jcode materially ahead (by design)
jcode owns a true in-process wake loop (runner.rs:532) gated on a PID lock and
pause-on-active-session; coco-rs's closest pieces (`TaskManager`,
`DreamService`) are reactive, and cron firing is pushed to a remote backend
(NoOp by default). Resource trade: jcode pays a continuously-resident server +
state files; coco-rs pays nothing when idle. For a Claude-Code-faithful tool
this is the correct trade — Claude Code has no ambient agent — but it is a
genuine capability gap if "the agent works while you sleep" is desired. This
**structural** gap is distinct from the (dead) forecast scheduler below.

### 2. Adaptive, rate-limit-aware scheduling — refuted at the live-behavior level
The forecast math is real code but **dead in the running system** (see Rejected
section). In practice jcode runs a fixed-max-interval + failure-backoff
scheduler. coco-rs has no future-work scheduler either, but porting an unused
forecast engine has no demonstrated value.

### 3. Self-dev — jcode is in a different category
The most impressive jcode subsystem; no coco-rs analog. Smoke-test-before-symlink
(reload.rs:289 vs :310) + rollback on every failure branch + cross-session
recovery continuation (reload.rs:101-111) make it safe rather than a gimmick.
**Conflict note:** this is fundamentally incompatible with coco-rs's faithful
Claude-Code-port identity and its single-fixed-binary / no-`unsafe` posture. It
is **out of scope**, not merely unimplemented. The *defensive* idea
(boot-validate-then-activate + rollback) is portable to coco-rs's swap surfaces —
see M11-S5.

### 4. Semantic skill injection — narrower delta than it first appears
Both surface un-invoked-but-relevant skills proactively. jcode ranks by
**embedding cosine similarity** (memory.rs:608-639); coco-rs ranks by **lexical
keyword overlap** (reminder_source.rs:117-170). The real delta is *ranking
quality*, not capability — and crucially the TS Claude Code coco-rs mirrors uses
a **Haiku LLM call**, not embeddings (audit_add.rs:11-27), so embedding-based
skill retrieval is a jcode-lineage choice, not a Claude-Code feature coco-rs
dropped.

### 5. Overnight bounded autonomy + structured morning report — jcode better, portable
jcode's supervisor (overnight.rs:229-429) is a clean prompt-phase machine with
periodic resource sampling, a re-rendered `review.html`, and evidence-bearing
task cards. coco-rs has no time-boxed autonomous run and no morning-report
artifact. The pattern (prompt-phases + JSONL events + structured task card) is
portable on coco-rs's background-agent + task-list layer **without** an always-on
server, because it is bounded and user-initiated — see M11-S3.

### 6. Browser & dictation — jcode ships built-in; coco-rs defers to MCP/v2
jcode's in-process browser (browser.rs) and dictation→`wtype` path are
lower-latency and zero-setup-per-session. coco-rs intentionally pushes browser
automation to MCP and treats voice as a v2 crate, consistent with Claude Code and
avoiding hard-coding a single browser provider. A defensible product divergence,
not a defect.

---

## Where coco-rs already matches or wins

**Auto-dream consolidation is more robust than jcode's ambient memory pass.**
- *Correct cancellation*: RAII `ConsolidatingGuard` (AtomicBool) + `LockGuard`
  roll back both the in-process flag and the lock mtime on a cancelled/failed
  future (dream.rs:236, 432-446), so a dropped future can't wedge subsequent runs
  or reset the 24h cadence. jcode's post-cycle embedding backfill is fire-and-
  forget (`tokio::spawn`, runner.rs:765-783) with no rollback.
- *Race-proof dual lock*: PID+mtime CAS file lock **and** a process-local atomic
  specifically to stop a manual `/dream` racing an auto-dream (dream.rs:236).
  jcode's `AmbientLock` is a single PID file (runner.rs:695).
- *Fenced subagent*: the consolidation fork is double-fenced
  (`AgentSpawnConstraints.allowed_write_roots` + a `create_auto_mem_handle`
  canUseTool permitting only Read/Glob/Grep, memdir-scoped Edit/Write, and
  known-safe Bash). jcode's ambient agent runs with broader ambient tools.

**Concurrency-safe, event-emitting task layer with clean DTO separation.**
`TaskManager` splits serializable `rows` from runtime-only `controls`
(running.rs), so the wire DTO never drags `CancellationToken`/`JoinHandle` Arcs
into transcripts or SDK output, and it emits typed `CoreEvent::Protocol` via an
opt-in sink. jcode's ambient persistence is whole-file JSON rewrites.

**Typed, multi-provider scheduling tools.** coco-rs's cron/trigger tools are
typed (`CronCreateInput`, `RemoteTriggerAction` enum) with validation at the tool
boundary and provider-agnostic routing through `ScheduleStore`
(scheduling.rs / schedule_store.rs). jcode's scheduling is Anthropic/OpenAI-OAuth-
centric.

**Broader, more faithful third-party extensibility surface.** 27-event hooks with
SSRF guard + scoped priority (`hooks/`), a full plugin system with marketplace +
hot-reload (`plugins/`), and managed/enterprise skill provisioning. jcode's
self-dev is a deeper *self-modification* story, but for *third-party*
extensibility coco-rs's hooks+plugins+managed-skills is wider and matches Claude
Code.

**Claims that don't hold for this module.** The README's "1800× faster mermaid",
"1000+ fps", "14ms TTFF" are TUI/render claims with zero bearing on
ambient/overnight/self-dev and are not exercised by any code path here. Separately,
`docs/AMBIENT_MODE.md:3` is marked "Status: Design" yet the runner/scheduler/tools
are fully implemented — the doc header *understates* implementation maturity (the
opposite of marketing inflation), which is why this analysis judged the live wiring,
not the doc.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only confirmed/nuanced suggestions appear here; the refuted one is in the next
section. Corrections from the adversarial pass are folded in.

### M11-S3 (confirmed) — Bounded overnight/long-run supervisor with structured task cards + morning report
**Why.** jcode's `run_supervisor` (overnight.rs:229-380) is a live phase machine
(preflight→coordinate→handoff→morning-report→grace→wrap-up) with 5-min resource
sampling + 30-min long-turn notices (run_turn_monitored, overnight.rs:382-429), a
mandated evidence card (`write_task_card_schema`, overnight.rs:970-1018:
why_selected/before/after/validation), and a re-rendered review artifact. coco-rs
has durable `task_list.rs` plan items but **no** phase-driven supervisor, **no**
duration cap, **no** evidence schema, **no** morning-report artifact (zero
non-test `overnight` hits).
**Concrete change.** Build a user-initiated, duration-capped
`coco-tasks::supervised_run` on top of the existing background-agent layer that
injects coordinator→handoff→wrapup prompts (mirroring `jcode-overnight-core`
builders). Refinements from review: (1) store each task card in the **existing**
`Task.metadata` JSON blob (`tasks/src/task_list.rs:81`, with `metadata_merge`),
not a new task-cards directory; (2) emit phase transitions + final report through
`CoreEvent::Protocol` (the engine already emits TaskStarted/Completed) — do not
invent a separate event stream; (3) honor cancellation by re-reading state each
turn via the `CancellationToken` already threaded through query (jcode reloads the
manifest every loop at overnight.rs:265); (4) **decouple from M11-S1** — bound the
run with a simple max-turns/max-duration/max-tokens cap via the existing
`BudgetTracker` (`app/query/src/budget.rs`), not a usage forecast.
**Layer.** `tasks/` + `app/query` (driver), events via `coco-types::CoreEvent`.
**Impact / effort / risk.** Medium / high / medium. Long unattended runs need a
real per-turn cancellation re-read to stay interruptible. **Non-goal check:** fits
coco-rs layering — it is bounded and user-started, so it needs **no** always-on
daemon and does not reintroduce ambient autonomy. Recommend a design doc first
(per the analysis-before-implementation convention).

### M11-S2 (nuanced) — Upgrade skill_discovery ranking from lexical to semantic, gated behind `Feature::Retrieval`
**Why.** jcode's embedding-backed skill recall is live (memory.rs:608-639:
`as_memory_entry` + `synthetic_skill_entries` + `ensure_embedding`, scored by
cosine similarity in the real recall flow). coco-rs **already** surfaces
un-invoked-but-relevant skills proactively (`skill_discovery`,
reminder_source.rs:117-170, capped at 5, into the system-reminder pipeline) — the
gap is **ranking quality** (lexical keyword overlap vs semantic), **not** a
missing injection path. Note the TS source uses a Haiku LLM call, not embeddings
(audit_add.rs:11-27), so this is an enhancement beyond Claude-Code parity.
**Concrete change (corrected scope).** Upgrade the **existing**
`SkillsSource::skill_discovery` producer's ranking — do **not** add a parallel
path. Two faithful options, in priority order: (a) **TS-parity** — wire
`skill_discovery` to a `Memory`/`Fast`-role side query (mirrors TS `prefetch.ts`
Haiku call), preserving the mirror-Claude-Code posture; (b) **embedding** — only
when `Feature::Retrieval` is on AND `coco-retrieval`'s vector index is already
warm, embed `(name + description + when_to_use)` and rank top-K via the existing
recall ranker (no new model call). Either way reuse the 5-cap dedup against the
always-listed budget, and keep the lexical heuristic as the zero-cost fallback.
**Layer.** `skills/` (producer) + `core/system-reminder` (consumer slot exists);
embedding path via `retrieval/` / `memory::MemoryRuntime::recall`.
**Impact / effort / risk.** Medium / medium / low. Risks: embedding cost per
skill at load, double-listing against the budget, ranker latency — all mitigated
by reusing the existing recall ranker and the dedup cap. **Non-goal check:**
respects non-goals; zero-cost when retrieval/LLM-discovery is off.

### M11-S4 (confirmed) — Terminal-result-required continuation for forked/background agents
**Why.** jcode guarantees a background cycle terminates cleanly: on a stop
without `end_ambient_cycle` it injects exactly one continuation
(runner.rs:937-942), re-checks (runner.rs:945), then emits a forced
`CycleStatus::Incomplete` with the transcript preserved (runner.rs:960-971),
logging "forced end after 2 attempts" (runner.rs:959). coco-rs has no equivalent
for forked/background agents: `DreamService` only records `DreamOutcome::Failed`
on subagent error (dream.rs:443-446) with no nudge; `forked_agent.rs` carries
`stop_reason` as a passthrough only; `tasks/src/stall.rs` is a *shell-output-
frozen + interactive-prompt* detector (stall.rs header + `looksLikePrompt`),
explicitly **not** an agent-didn't-finish detector.
**Concrete change (corrected scope).** Add a terminal-result-required policy for
**framework-spawned forks + `run_in_background` subagents only** (the interactive
main loop already has `stop_reason` recovery and must not get a second policy). On
"agent loop ended with empty/non-terminal result and no expected completion
signal", inject **exactly one** bounded continuation message (analogous to
`ReloadContext::interrupted_session_continuation_message`), then close with an
explicit `Incomplete` status and persist the partial transcript. Reuse
`BudgetTracker.max_continuations` + `record_continuation`
(`app/query/src/budget.rs:27, 66`) to cap at one retry — do **not** add a new
counter. The trigger is the empty/non-terminal-result condition, **not**
`stall.rs` (which is shell-specific).
**Layer.** `tasks/` + `app/query` (fork/background close path).
**Impact / effort / risk.** Low / low / low. Must cap to exactly one retry to
avoid runaway spend. **Non-goal check:** respects non-goals.

### M11-S5 (nuanced) — Pre-activation validate + rollback guard for live component swaps (defensive primitive, NOT self-dev)
**Why.** jcode's self-dev validates a new build by booting it before flipping the
live pointer and records a rollback-able `PendingActivation`
(`build::smoke_test_server_binary` at reload.rs:289 **before**
`update_shared_server_symlink` at :310; `rollback_pending_activation_for_session`
on every failure branch at :312/:330/:348/:381/:391). coco-rs has swap surfaces
with **no** boot-validate-then-activate + rollback guard — but the original two
cited targets are **uneven**, so the scope is corrected.
**Concrete change (corrected targets).**
- *(a) PRIMARY — plugin hot-reload.* Gap confirmed: `plugins/src/hot_reload.rs`
  is just a `PluginReloadTracker` `AtomicBool` (hot_reload.rs:33-57); the
  contribution swap is documented "clear old + register new" with **no**
  pre-swap validation (`plugins/src/hook_bridge.rs:6-7`,
  `register_deduped` at hook_bridge.rs:88-99), so a malformed reloaded plugin can
  swap in broken hooks. Before `hook_bridge`/`command_bridge`/`skill_bridge` swap
  in a reloaded plugin's contributions, validate the new `PLUGIN.toml` +
  contribution set (schema parse + the `validatePlugin`-equivalent) and keep the
  prior `LoadedPlugin` set for atomic rollback on validation failure.
- *(b) SECONDARY — MCP reconnect.* `services/mcp/src/client.rs:585`
  `spawn_reconnect` / `:594` `spawn_reconnect_after_oauth` re-register tools via
  `manager.connect()` with no capability probe beyond the connection succeeding.
  Gate tool re-registration on a successful `tools/list` and retain the prior tool
  set until the probe succeeds. (`connect` already calls `list_tools` during init
  at client.rs:291, so the probe data is largely present — formalize it as a
  swap gate.)
- *(c) DROP — hub/server.* The hub crates do **no** live component swap today
  (EventStore read model + Axum server), so there is nothing to guard. Revisit
  only when the concurrent-app-server work introduces a runtime component swap.
**Layer.** `plugins/` (primary) + `services/mcp` (secondary).
**Impact / effort / risk.** Low / medium / low (pure infra hardening; per-surface
cost is defining the validate/probe step). **Non-goal check:** this is the
*safety* idea from self-dev, **explicitly not** self-modification — coco-rs stays a
single fixed binary. Respects non-goals.

### Additional recommendation from verifier missed-findings — Per-run autonomous transcript artifact for background runs
**Why.** jcode persists a structured `AmbientTranscript` per cycle (session id,
started/ended, status, provider/model, summary, **full conversation**, pending
permissions, memories_modified, compactions; runner.rs:739-759) and dispatches a
cycle-summary notification (runner.rs:762). coco-rs background tasks emit
`TaskStarted/Progress/Completed` events but persist **no** equivalent per-run
artifact for an autonomous/background run, so a completed background run leaves no
durable, reviewable record beyond ephemeral events.
**Concrete change.** When the M11-S3 `supervised_run` (or any `run_in_background`
agent) completes, write a structured run record into the `Task.metadata` blob
(`tasks/src/task_list.rs:81`) — status, duration, model identity from the
per-(provider, model) `UsageAccumulator` (`services/inference/src/usage.rs:18`),
summary, and a transcript pointer — and emit one terminal `CoreEvent::Protocol`
carrying the summary. This piggybacks on M11-S3 and the existing event/usage
plumbing; no new subsystem.
**Layer.** `tasks/` + `app/query`. **Impact / effort / risk.** Low / low / low.
**Non-goal check:** respects non-goals (bounded, user-initiated runs only).

---

## Rejected after adversarial review

### M11-S1 (refuted) — "Usage-forecast-aware interval scheduler for autonomous/recurring runs"
**The jcode claim does not hold at the live-behavior level.** The forecast math
the suggestion cites (`scheduler.rs:188-247`: `user_rate` projection,
`user_budget_reserve=0.8`, `ambient_budget / tokens_per_cycle`) is **real code
but dead in the running system**. Three independent source confirmations:
1. `usage_log.record(...)` is **never called outside `scheduler.rs`'s own
   `#[cfg(test)]` block** — `grep '\.record(' src/ambient` returns nothing in
   non-test code, so the rolling `UsageLog` the whole forecast depends on is
   **always empty** in production.
2. The live runner calls `scheduler.calculate_interval(None)` at **both**
   `runner.rs:666` and `runner.rs:799` — both arguments are literal `None`. The
   only `calculate_interval(Some(..))` calls are at scheduler.rs:382/404/432/434,
   **all tests**.
3. `RateLimitInfo { .. }` is only ever constructed at scheduler.rs:375/396/424,
   **all tests**.

With `None` + an empty log, `calculate_interval` returns `apply_backoff(max)`
(scheduler.rs:195) — i.e. the live scheduler degenerates to a **fixed
max_interval (default 120 min)** modulated only by the ×2..×64 exponential
backoff (`on_rate_limit_hit` at runner.rs:787, `on_successful_cycle` at
runner.rs:736, both of which *are* wired). So jcode in practice runs exactly the
"fixed cron + failure backoff"-class behavior the suggestion claimed it
transcends. The "rolling per-source token-usage forecast + rate-limit headroom"
is aspirational/unit-tested scaffolding, not a live mechanism.

**Why not port it anyway.** Porting an unused forecast engine copies untested
scaffolding with no demonstrated value on either side. coco-rs already has the
raw ingredients (`UsageAccumulator.per_model` splits by (provider, model),
usage.rs:18-43; generic retry/backoff in `services/inference/src/retry.rs`).
Additionally, rate-limit data lives in `vercel-ai-<provider>` per the documented
non-goal — building a per-source forecast against `RateLimitInfo` would also
cross a layer boundary for no proven benefit.

**What survives.** Only the **failure-backoff state machine** is live and
worth keeping in mind: `on_rate_limit_hit → ×2 (cap ×64)` /
`on_successful_cycle → reset`, clamped to [min, max] (scheduler.rs:260-276). If
the M11-S3 bounded recurring runner is ever built, give *it* a simple per-run
backoff multiplier reset-on-success — not a per-source token forecast.
