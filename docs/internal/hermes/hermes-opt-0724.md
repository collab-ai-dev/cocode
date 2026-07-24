# coco-rs 0724 Optimization Plan — Hermes Absorption Refresh

Status: analysis complete (2026-07-24) — supersedes the priority ordering in
[hermes-opt.md](hermes-opt.md) (2026-07-10); that document remains the
evidence record for the v0.2.0→v0.18.2 window.
Related: [plans/](plans/README.md) (per-item dev plans from the 07-10 sweep —
per-plan validity re-stamped in §7 below),
[README.md](README.md) (architecture-level comparison, 2026-07-02),
[../tool-result-offload-v2-design.md](../tool-result-offload-v2-design.md) (shipped),
[../cc-catchup-roadmap-v2.md](../cc-catchup-roadmap-v2.md) (model-card overlap).

## 1. Scope & Method

- **Hermes delta analyzed:** `a7f65e3bc` (the 07-10 sweep anchor,
  `v2026.7.7.2`+161) → `ef6ce56ca` (HEAD, 2026-07-24) — **2,487 commits**,
  consisting of **v0.19.0 "Quicksilver"** (`v2026.7.20`, ~2,245 commits,
  ~1,065 PRs, rolls up v0.18.1/v0.18.2) plus **961 untagged post-release
  commits**. Sources: full v0.19.0 GitHub release notes (~53 KB) read in
  entirety + post-tag commit log clustered by scope + targeted source reads
  in the hermes checkout. Hermes citations below are repo-relative paths
  pinned to commit **`ef6ce56ca`**.
- **coco-rs side re-verified from scratch:** branch `feat/hermesdoc`
  (HEAD `f5f2dfae`, 2026-07-24) by five read-only exploration passes —
  every 07-10 verdict re-checked (much has shipped since: tool-result
  offload, goal runtime, journey, rtk output filter, MCP tool-exposure
  unification, session content search, Grep grouping) and ~25 new
  candidates from the delta checked with file:line evidence.
- **Skipped categories** (unchanged from 07-10): desktop/dashboard chrome,
  messaging-platform adapters, installers/packaging, model-catalog
  additions, Python-specific fixes. Hermes's Nous-subscription billing
  (`/subscription`, `/topup`) is skipped as vendor-specific; the ~80% TTFT
  cold-start war is skipped as Python-specific (coco's Rust startup and
  background MCP connect already cover the transferable part — see §3).

Where the v0.19 window concentrated (agent-core-relevant axes only):

1. **Compression hardening, round 2** — proactive pruning for large-window
   models with a prompt-cache reclaim gate, summarizer-input bounding,
   N-user tail preservation, ghost-skill defense, breaker persistence.
2. **MoA robustness** — a ~30-commit hardening wave (interrupt, timeout,
   per-advisor windows, per-slot controls, all-fail paths).
3. **Durability ledgers** — delegation completions and final-response
   delivery survive process death.
4. **Approval ergonomics** — smart approvals by default, deny-with-reason,
   deny rules that outrank yolo.
5. **Cross-provider replay fidelity** — Gemini `thoughtSignature` sentinel,
   empty-text-block coercion, tool_call_id dedup.

## 2. Status of the 2026-07-10 List (re-verified 07-24)

| 07-10 item | 0724 status | Evidence (coco `feat/hermesdoc`) |
|---|---|---|
| P0.1 Compact summary language rule | **STILL OPEN** | zero "language" hits in `services/compact/`; templates unchanged (`prompt.rs:92,176,240`) |
| P0.2 Compact temporal anchoring | **STILL OPEN** | no date injection / past-tense rule in `services/compact/` |
| P0.3 Empty-response nudge/retry | **STILL OPEN** | empty finish still ends turn (`app/query/src/engine_terminal.rs:335-346`); thinking-only text dropped (`engine_stream_consume.rs:601`). Reusable in-repo patterns: max_tokens resume-nudge (`engine_recovery.rs:635-640`), budget-continuation nudge (`engine_terminal.rs:389`) |
| P0.4 Warning-first loop guardrail | **STILL OPEN** | `inputs_equivalent` still has zero call sites (`core/tool-runtime/src/traits.rs:308,771,1065-1072`); no `loop_guardrail` config |
| P0.5 Edit closest-match feedback | **STILL OPEN** | bare `old_string not found` at `core/tools/src/tools/edit.rs:411,443`; `find_fuzzy_match:518` is apply-only, never surfaces a near-miss |
| P0.6 Bash ANSI stripping | **NARROWED** | rtk-filtered path strips (`exec/shell/src/rtk/filter.rs:156-158`; `Feature::OutputRewrite` default-on, `common/types/src/features.rs:373-378`); **unfiltered/raw path still un-stripped** (`core/tools/src/tools/bash.rs:991-993`). Remaining work: strip on the always-path |
| P0.7 Micro-compact recovery pointer | **CLOSED** ✅ | offload shipped (`5ca6a0ce`); micro-compact preserves pointers (`services/compact/src/micro.rs:89`, `micro_advanced.rs:83`, `types.rs:107-179`) |
| P1.1 MCP `list_changed` + keepalive | **STILL OPEN — now cheaper** | handler still log-only (`services/rmcp-client/src/logging_client_handler.rs:96-98`); but `refresh_server_capabilities` now exists with no production caller (`services/mcp/src/discovery.rs:222`) — the refresh half is wiring, not building. Keepalive still absent |
| P1.2 ToolSearch deferral threshold | **STILL OPEN** | `should_defer()` still a static bool (`core/tool-runtime/src/traits.rs:626-628`, `registry.rs:646-658`); the ca341b75 `McpToolExposure::{Load,Defer,UseTool}` enum is categorical, not size-gated |
| P1.3 Grep densification | **CLOSED — better than spec** ✅ | `format_content` → `group_content_blocks`, path printed once per file + per-file caps + overflow markers (`core/tools/src/tools/grep.rs:933,984,1074`); unconditional (no ≥5 gate needed) |
| P1.4 Zero-LLM cron scripts | **STILL OPEN** | `CronCreateInput` unchanged (`core/tools/src/tools/scheduling.rs:80-102`); every fire enqueues an agent turn (`app/agent-host/src/integrations/cron_tick.rs:177` — note the file moved from `app/cli`) |
| P1.5 Session full-text search | **MOSTLY CLOSED** ✅ | picker-level transcript content search shipped: `app/session/src/lib.rs:366` (`search_content`, linear scan in `spawn_blocking`) + TUI wiring. Residual (only if demanded): model-facing search tool; FTS/trigram index only if linear scan proves slow — deterministic-by-construction honored |
| P1.6 Model retirement metadata | **STILL OPEN + doc drift** | `ModelCard` has no deprecation/retirement field (`common/model-card/src/schema.rs:3-18`); root `CLAUDE.md` claims "deprecation" the schema doesn't have — fix the doc when (not if) the field lands |
| P1.7 Reasoning stall-timeout floor | **PARTIAL** | per-provider opt-in `stream_idle_timeout_secs` landed (`services/inference/src/client.rs:309`, `model_factory.rs:263`); default still `.without_idle_timeout()` + 20 s soft stall-warn (`stream.rs:614-615`). Remaining: per-model floor (model-card home) |
| P2.1 `/goal` judged loop | **BUILT — differently; re-scoped** | first-class goal runtime shipped (`0dacc81a`): durable `GoalSnapshot`, deterministic `GoalCompletionCoordinator` (fail-closed, not fail-open), 20-turn default budget (`core/goals/src/budget.rs:15`), `/goal pause|resume`, promptless autonomous turns + per-turn reminder suffix (cache-prefix intact). **Do not build the hermes Ralph loop** — close the completion gaps instead (§4 N10) |
| P2.2 Fork token discipline | **CLOSED by different design** ✅ | skill-learn fork achieves cache parity via Arc-shared byte-identical message prefix (`app/query/src/fork_context.rs:11-19`) instead of hermes's digest; cadence = signal gate + throttle + failure backoff (`skill-learn/src/runtime.rs:172-179`). Residual micro-audit: hermes `#64379` found reasoning-config mismatch breaks Anthropic cache parity — check the fork's thinking config when `ModelRole::Memory` resolves to the parent's model |
| P2.3 Browser automation | **STILL UNDECIDED** | hermes keeps compounding (browser snapshots-on-truncation `#65923`, computer_use verify→escalate ladder `#67123`). The go/no-go call is still owed; every sweep re-finds it |

## 3. Newly Verified Present in coco — Do NOT Re-Flag

New equivalences found while checking the v0.19 delta (beyond §2's carried
table):

| Hermes v0.19 mechanism | coco equivalent (evidence) |
|---|---|
| `/deny <reason>` — denial reason reaches the model (`#54518`) | TUI deny-reason input ships as `ApprovalResponse.feedback` → `ToolResultContent::ExecutionDenied{reason}` (`app/tui/src/state/surface_payloads.rs:69-72`, `app/query/src/engine_prompt.rs:1048`) |
| User deny rules block even under yolo (`#59164`) | deny rules are step 1 for **all** modes; `BypassPermissions` auto-allow is only the step-8 fallthrough (`core/permissions/src/evaluate.rs:148-176,640-644`) |
| Live subagent transcripts, tail-able (`#67479`) | background subagents append per-child JSONL at `<sid>/subagents/agent-<id>.jsonl`, read back by `agent/resume` (`coordinator/src/agent_handle/spawn.rs:2311-2377`) |
| MoA: aggregator-alone when all references fail; fan-out progress (`f0ed77b62`, `#59546`) | failures become `[failed: …]`/`[empty]` markers, aggregator still runs (`app/query/src/moa.rs:217-234,551-588`); `MoaReferenceStarted/Completed/Aggregating` + per-reference thinking blocks (`moa.rs:298-407`) |
| Flatten multimodal for the summarizer (`#65046`) | images/documents → `[image]`/`[document]` text incl. inside tool_results (`services/compact/src/compact.rs:629-725`) — caveat: assistant-message File parts fall through (`:723`, §4 N3b) |
| Ghost-skill defense (`[SKILL_PRUNED]` marker) | **stronger by design**: micro-compact never clears SkillTool results (`services/compact/src/micro.rs:16-26`) and full compact re-injects invoked skills post-summary under budgets (`post_compact_skills.rs:41-66`, `types.rs:77,80`) — content survives instead of a removal marker |
| Todo-snapshot merge avoiding user/user adjacency (`d2bb6cc25`) | post-compact rehydration (`app/query/src/engine_turn_reminders.rs:419-469`) + `MergeConsecutiveUsers` at API-normalize (`core/messages/src/normalize.rs:143-153`) |
| Session export (`#60186`) | `/export` Markdown/JSON/Text from live history (`app/agent-host/src/session/conversation_export.rs:12-45`) — format/headless expansion is §4 N13, the core exists |
| First-turn latency: capability probes off the critical path (`#59332`) | MCP connects in background by default (`SessionMcpConnectMode::Background`, `app/agent-host/src/session/session_bootstrap.rs:150-154,700-729`); headless awaits deliberately; skill/plugin scan is `spawn_blocking` + per-project cached (`session_runtime/factory.rs:296-326`) |
| `max`/`ultra` effort tiers; per-model effort pins (`#62650`, `#64458`) | `ReasoningEffort` already 6 levels; per-role-slot `effort` + `ProviderModelOverride.overrides.default_thinking_level` (`common/config/src/model/role_slots.rs`, `provider/model_override.rs:25,34`); mid-session Ctrl+T / F2 cycling |
| Smart approvals (LLM reviewer for flagged commands, `#62661`) | the 2-stage XML classifier exists and is finer-grained (per-call); it is **opt-in via Auto mode** (`app/query/src/tool_call_preparer.rs:458,499-503`) where hermes made it default — a product decision, tracked as §4 N17, not a gap |

## 4. New Absorption Items (all verified against coco 07-24)

### P0-N — cheap, direct wins (ride the existing P0 PRs)

**N1. Empty text-*part* coercion on the Anthropic request path — ABSENT.**
Hermes `4c9628eab` (#69512/#69517) coerces empty/whitespace-only text blocks
at request build. coco's normalize covers whole messages
(`core/messages/src/normalize.rs:93-141`), but the convert path pushes
part-level empties verbatim inside mixed content: `convert_user_part`
emits `{type:"text",text:""}` (`vercel-ai/anthropic/src/messages/convert_to_anthropic_messages.rs:506-515`),
assistant text is trailing-trimmed then pushed even when empty (`:686-701`);
only an entirely-empty content array elides the message (`:288-293`).
Anthropic 400s on empty text blocks. Fix: skip empty text parts at convert
(provider crate — correct layer). Size XS; ride PR-2.

**N2. Full-compact keep-tail — expose as config, default stays 0
(decided 2026-07-24).**
Hermes `a9c868225` preserves the most recent N user messages through
compression, and `d43cc2ca8` gates the guarantee to *actionable* turns
(`agent/context_compressor.py`, `min_tail_user_messages`). coco has the
crate-level knob (`keep_recent_rounds`,
`services/compact/src/compact.rs:70,181-183`) but production full compact
passes `..Default::default()` = 0
(`app/query/src/engine_compaction_full.rs:346-352`), and the knob is not
reachable from settings. **Claude Code parity check (2026-07-24, against
`claude-code` TS source):** TS full compact has NO such parameter and
keeps zero verbatim messages — `compactConversation` summarizes the whole
history (`src/services/compact/compact.ts:387`, `messagesToSummarize =
messages`; `messagesToKeep` exists only in the message-selector
`partialCompactConversation` path) and compensates via the post-compact
attachment bundle, which coco also ports. So 0 IS TS parity, and flipping
the default would be a deliberate divergence toward hermes.
**Decision: default stays 0; expose the knob.** Work: add
`keep_recent_rounds` to `PartialCompactSettings`/`CompactConfig`
(`common/config/src/settings/mod.rs:170`, `compact_settings.rs:157`,
serde default 0) and thread it into the `engine_compaction_full.rs`
construction site. Note `coordinator/src/agent_handle/teammate_engine.rs:229`
already sets a non-zero `KEEP_RECENT_ROUNDS_FOR_FULL` — keep that path's
explicit value winning over the config default. Hermes's actionable-turn
gate becomes relevant only if the default ever flips; not in scope. Size XS–S.

**N3. Proactive summarizer-input bound — reactive-only today.**
Hermes bounded the compression summary input up front (`80ece3867`,
re-anchored `b7a05b6b6`). coco sends the **entire** old-rounds slice to the
summarizer (`services/compact/src/compact.rs:199-216`) and recovers only
reactively — 3 PTL retries dropping head rounds after the API rejects
(`compact.rs:986-1095`). On a very long history that is up to 3 wasted
summarizer calls. Fix: pre-truncate with the existing
`truncate_head_for_ptl_retry` machinery (`:881-941`) using the token
estimator before the first call. Size S.
**N3b (ride-along, XS):** media strip pass skips `Message::Assistant`
File/ReasoningFile parts (`compact.rs:723`) — close the traversal gap.

### P1-N — medium cost, reliability wins

**N4. Proactive tool-result prune for large-window models + min-reclaim
cache gate — the headline compression idea of the window.**
Hermes `cb481e2f2`: the no-LLM tool-result prune only ran inside
compress() (~50% trigger) so it *never fired on large-window models*; old
tool outputs ride in history and are re-billed every turn. They added
`prune_tool_results_only()` on a separate low token trigger
(`compression.proactive_prune_tokens`, opt-in). Hermes `fa4800414` added the
key insight: `proactive_prune_min_reclaim_tokens` (default 4096) — a prune
only **commits when it reclaims a meaningful batch**, because rewriting
already-sent history invalidates the provider prompt-cache prefix; the
hysteresis keeps cache breaks episodic/amortized like a compression
boundary (`agent/context_compressor.py`). coco today: auto-compact at ~87%
of window (`services/compact/src/auto_trigger.rs:451-465`), time-gap
micro-compact (`app/query/src/engine_finalize_turn.rs:542-587`) — nothing
token-proportional for 1M-window models, and **no reclaim-size gate**
anywhere. Fix: opt-in token-threshold micro-compact trigger + min-reclaim
hysteresis in the micro path; composes cleanly with offload (clearing a
pointer-bearing result keeps the pointer — recovery preserved). Size M.

**N5. Pre-send preflight compact — Block today, should compact.**
Hermes wired `should_compress_preflight` into turn-start (`929c95259`).
coco's pre-send gate `check_blocking_limit` only **blocks** the turn
terminally (`app/query/src/engine.rs:721-733`; decision `Block`,
`engine_recovery.rs:197,381`); client-side overflow is otherwise handled
after the API rejects. Fix: at the pre-send gate, when the assembled
request exceeds budget, run compaction and continue instead of dying.
Size S–M (the compaction entry points all exist).

**N6. Gemini `thoughtSignature` sentinel for cross-provider tool calls —
squarely in coco's multi-provider lane.**
Hermes `8d119832b` emits a sentinel thoughtSignature when replaying
tool_calls that originated on another provider into the Gemini native
adapter (`agent/gemini_native_adapter.py`; companion MoA fix
`f65d105cb`). coco simply **omits** the field: signature is read only from
`google`/`vertex` provider metadata (`vercel-ai/google/src/convert_to_google_generative_ai_messages.rs:201-219,306-317`)
and `skip_serializing_if = Option::is_none`
(`google_generative_ai_prompt.rs:64-69`). Replaying an Anthropic/OpenAI-
originated tool call into a thinking-enabled Gemini request therefore
ships a functionCall with no signature — exactly the class of
cross-provider 400/degradation coco's Anthropic-side hygiene already
guards against (that direction was verified present on 07-10). Fix in the
provider crate; verify live behavior first (wire-dump a cross-provider
handoff), then emit the sentinel hermes uses. Size S.

**N7. MoA hardening batch.** Hermes spent ~30 commits here post-tag; coco's
MoA (`app/query/src/moa.rs`) verified gaps:
- (a) **No interrupt abort**: references fan out via `join_all` (`:238`)
  with no cancel token; hermes aborts the wait on user interrupt
  (`68cd75573`) and closes the stream on interrupt (`8d14e19f9`).
- (b) **No per-reference timeout**: only 3 retry attempts (`:42,530-549`);
  hermes defaults reference_timeout from the auxiliary config
  (`d3fc27bbf`).
- (c) **No per-advisor window trim**: fixed byte budgets
  (`REFERENCE_GUIDANCE_TEXT_BUDGET=24_000`, `:43-44`); hermes trims
  reference messages to *each model's* context window (`975eb3a36`).
- (d) **No per-slot controls**: `thinking_level = None` hardcoded for all
  references (`:188`); `reference_max_tokens`/`temperature` are per-preset
  only (`common/config/src/model/moa.rs:45-49`); hermes has per-slot
  effort + max_tokens + enable toggles (`280c4dce7`, `bc7212cf9`,
  `ca294d3e6`).
- (e) **Prompt hardening worth copying verbatim**: explicit warnings in the
  reference prompt against advisors *claiming tool execution*
  (`6afbb33af`) — coco advisors are tool-less too and can confabulate the
  same way.
Size M as a batch; (a)+(b) are the reliability half, (c)+(d) the quality
half.

**N8. Hook output cap + spill — unbounded today.**
Hermes spills oversized hook-injected context to disk (`#20468`). coco
reads hook stdout with unbounded `read_to_string`
(`hooks/src/lib.rs:1294`, stderr `:1334-1345`) and collects
`additional_context` without any size limit
(`hooks/src/orchestration.rs:1080-1081,1149`) — a misbehaving hook can
blow the context in one shot. Fix: byte cap + spill through the existing
persisted-output/offload seam (pointer in context, full text on disk).
Size S–M; the offload vocabulary makes this nearly free.

**N9. Durable background-job ledger — the seam exists, wire it.**
Hermes made background delegation completions durable via an
ownership-checked ledger, restored on restart (`#63494`,
`tools/async_delegation.py`), and the same idea guards final-response
delivery (`#67181`, `gateway/delivery_ledger.py`). coco has `JobStore`
writing durable terminal records to `<config_home>/bg-jobs/` — but the
spawn/exit **write wiring is explicitly deferred and has zero callers**
(`tasks/src/job_store.rs:16-22,79-147`); running tasks are purely
in-memory (`tasks/CLAUDE.md`), only `coco ps` reads the store
(`app/cli/src/bin_handlers/ps.rs:51`). A restart mid-task silently loses
results. Fix: wire spawn/exit writes, reconcile on session open, notify
undelivered completions. Size M. **Synergy:** the goal runtime's
task-completion wake needs the same `TaskManager` subscription seam
(`goal_driver.rs:50-56`) — build once, serve both.

**N10. Goal completion hardening (re-scope of old P2.1 / plans/p2-1).**
The goal runtime shipped with the right skeleton (deterministic,
fail-closed, evidence-ledgered — arguably sounder than hermes's fail-open
judge). What remains is exactly the part hermes proved matters most
(verify-on-stop, v0.18 `#50501`/`#52285`: *never trust the model's claim
of done*):
1. **Execute deterministic checks for real** — `CheckPredicate::Command`
   exists but is never run; the boundary audit marks every requirement
   `satisfied: true` with empty evidence **without running anything**
   (`core/goal-runtime/src/coordinator.rs:225-266`). This is the
   highest-value gap: today's gate can pass on vibes.
2. **Model-backed `CompletionVerifier`** — live wiring passes the
   `AlwaysVerified` double (`app/agent-host/src/session/goal_driver.rs:401-403`);
   implement the aux-role verifier (crate has no inference dep yet — keep
   the seam, inject from agent-host).
3. **Task-completion wake** — via N9's subscription seam.
4. **Doc-only skip** (cost gate) and, later, subgoal layering
   (`/subgoal`, hermes `#25449`) if demand appears.
Size L total but stageable; stage 1 alone is S–M and closes the "claims
done without proof" hole.

**N11. Unknown-config-key warning — silent today at top level.**
Hermes warns on unknown root config keys + doctor reports deprecated keys
(`#65540`, `#67370`). coco's top-level `Settings` silently ignores unknown
keys (deliberate back-compat, `common/config/src/settings/mod.rs:67`)
while typed model/provider sub-blocks hard-reject and
hooks/permissions get targeted validation warnings
(`settings/validation.rs:489,546,710-763`). Fix: collect unknown top-level
keys during load and warn (never fail) — the validation.rs pattern already
exists. Size S. A typo'd `compact:` block today just silently does
nothing.

### P2-N — strategic / product decisions

**N12. Pluggable `SecretSource` (1Password/Bitwarden).** Hermes's
`agent/secret_sources/` interface resolves config secrets from multiple
vaults at load with deterministic precedence, conflict warnings, and
per-variable provenance (`#59498` — consolidated eleven community PRs, so
demand is real). coco has `keyring-store` only. Landing shape: a
`SecretSource` trait at config-resolution (env → keyring → vault refs à la
`op://…` in settings values), provider crates untouched. Needs a mini
design doc (provenance + failure semantics). Size M–L.

**N13. Session export expansion.** On top of the existing `/export`
(md/json/text, interactive-only): headless `coco sessions export`,
`--redact` via the already-shipped `secret-redact` crate, prompt-only and
trace formats, and compacted-session lineage stitching (hermes `#60186`,
`#59327`). The dataset/replay angle composes with `session-trace`. Size M.

**N14. Stacked slash-skill invocation.** `/a /b args` loads both skills in
order — a Claude Code port hermes made (`#57987`). coco parses a single
`(name, rest)` (`app/cli/src/tui/slash_resolution.rs:129-138`). Size S.

**N15. `/model --once`** — one-turn model override with auto-revert
(hermes `#67113`). `TurnStartParams` already carries a per-turn model at
the protocol level; only the slash surface is missing. Size S.

**N16. Compact breaker persistence + surfacing.** Anti-thrash and
failure-breaker state is in-memory only (`services/compact/src/reactive.rs:57-58,130-137`;
fresh `::new()` in `app/query/src/engine_builder.rs:134-141`) — a thrash
loop resumes after restart with a clean slate (hermes persisted theirs,
`ec5835ab8`). Also: the failure-breaker skip is only `warn!`-logged, never
surfaced to the user (`engine_finalize_turn.rs:810-818`) while the
rapid-refill breaker does surface — align them (hermes surfaces the
lock-hold reason on every no-op compress, `b86367e49`). Size S.

**N17. Smart-approval default-on — product decision, not code.** coco's
classifier is finer-grained than hermes's but requires switching into Auto
mode; hermes made LLM review of flagged commands the default posture
(`#62661`, `tools/approval.py`). Decide deliberately; if default flips,
the `auto_mode.classifier_unavailable_fail_open` semantics deserve a
re-read first.

**N18. Context-engine hooks — watch, no action.** Hermes added a pluggable
`select_context()` / `on_turn_complete()` pair on their ContextEngine ABC
(post-tag `dec464c35`, `bb9ef9d72`; fail-open, replace-request-only, no
persisted-history mutation). coco's equivalents (MessagePass pipeline,
trait-based system-reminder generators, compact strategy seams) cover the
internal needs; coco has no third-party context-engine product surface. Do
not build an extension point without a consumer (see anti-lesson 8 — the
adjacent hermes "memory provider-actions" extension point was reverted for
exactly this).

**N19. Browser/computer-use go/no-go — still owed** (carried from 07-10
§5.3 unchanged; hermes keeps compounding). Either schedule the design
effort or add it to explicit non-goals so it stops resurfacing.

## 5. Re-Prioritized 0724 Backlog

Sequencing (each PR independently shippable):

1. **PR 1 — compact prompt + input** (extends plans/p0-1): language rule
   (P0.1) + temporal anchoring (P0.2) + proactive summarizer-input bound
   (N3) + assistant-media strip (N3b). All inside `services/compact` +
   date threaded from the caller.
2. **PR 2 — loop robustness** (extends plans/p0-2): empty-response nudge
   (P0.3, with the `<think>`-leak regression pin — anti-lesson 9) + Edit
   closest-match (P0.5) + ANSI strip on the unfiltered bash path (P0.6
   narrowed) + Anthropic empty-text-part coercion (N1).
3. **PR 3 — guardrails** (plans/p0-3 as written): warning-first repeated
   tool-call guard on the `inputs_equivalent` seam.
4. **PR 4 — keep-tail config** (N2): expose `compact.keep_recent_rounds`
   in settings (default 0, CC parity) wired through `CompactConfig` +
   snapshot tests.
5. **P1 batch, value order:**
   N10-stage-1 (execute goal checks for real) →
   p1-1 (MCP `list_changed` wiring + keepalive) →
   N4 (proactive prune + reclaim gate) →
   N6 (Gemini signature sentinel) →
   N7a/b (MoA interrupt + timeout, then c/d quality half) →
   N5 (pre-send preflight compact) →
   N8 (hook output cap/spill) →
   N9 (job-ledger wiring; unlocks N10-stage-3) →
   p1-2 (ToolSearch threshold) → p1-4 (zero-LLM cron) →
   p1-6 (model retirement + fix the CLAUDE.md doc drift) →
   N11 (unknown-key warning) → P1.7 remainder (per-model floor).
6. **P2:** N12 (SecretSource, mini design doc first) → N13 (export) →
   N16 → N14/N15 → N17 (decision) → N19 (decision). N18 stays
   watch-only.

Dropped from the backlog as done: p1-3 (Grep), p1-5 core (session search),
old P0.7 (offload), old P2.2 (fork discipline). p2-1 is superseded by N10
— **do not** implement the hermes-style fail-open LLM judge loop over the
shipped fail-closed coordinator.

## 6. Anti-Lessons — additions from the v0.19 window

(1–5 from [hermes-opt.md §6](hermes-opt.md) still stand.)

6. **Workflow orchestration as a *skill* failed to land.** Hermes's
   dynamic-workflow orchestration skill was landed then reverted in the
   v0.19 window ("not shipping"). coco's engine-level
   `workflow`/`workflow-runtime` is the right layer — when skill-layer
   orchestration ideas resurface from hermes, they are not evidence coco's
   approach is missing something.
7. **Credential-injection egress proxy reverted.** iron-proxy (#30179,
   inject credentials at an egress firewall) was reverted before release.
   Keep coco's model: credentials live in provider crates, period.
8. **Extension points without consumers rot.** Hermes's memory
   provider-actions extension point landed and was reverted in the same
   window. Applies directly to N18.
9. **Thinking-only retry can leak `<think>` into visible output.** After
   hermes added the thinking-only-response retry flush, `<think>` content
   leaked into user-visible text and needed a community fix (@xxxigm,
   v0.19 credits). When implementing P0.3, pin a regression test that
   reasoning text never reaches visible output on the nudge/retry path.
10. **Don't impersonate other clients' OAuth UAs.** Anthropic OAuth login
    429'd hermes until the UA stopped claiming `claude-code/`
    (`#58178`). coco's provider-auth flows keep their own identity.

## 7. plans/ Validity Stamps (07-24)

| Plan | Verdict |
|---|---|
| p0-1-compact-prompt | **valid** — extend scope with N3 + N3b (same crate) |
| p0-2-loop-robustness | **valid** — ANSI item narrowed to the unfiltered path (rtk covers the filtered path); add N1 and the anti-lesson-9 regression pin |
| p0-3-tool-loop-guardrails | **valid as written** — seam unchanged, still zero call sites |
| p1-1-mcp-transport | **valid, cheaper** — `refresh_server_capabilities` now exists; the refresh half is a wiring task |
| p1-2-toolsearch-threshold | **valid** — re-read against the new `McpToolExposure` enum before coding |
| p1-3-grep-densify | **DONE — drop** (shipped unconditional, superset of the plan) |
| p1-4-zero-llm-cron | **valid** — cron_tick moved to `app/agent-host/src/integrations/cron_tick.rs`; update paths |
| p1-5-session-search | **mostly done — shrink** to the residual decision (model-facing tool? index?) |
| p1-6-model-lifecycle | **valid** — add the root-CLAUDE.md doc-drift fix; reasoning-floor half updated by P1.7 partial |
| p2-1-goal-loop | **superseded by §4 N10** — goal runtime exists with a sounder (fail-closed, evidence-gated) skeleton; absorb only the completion-hardening deltas |
