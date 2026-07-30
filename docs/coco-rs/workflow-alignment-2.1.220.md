# Dynamic Workflow — coco-rs vs Claude Code 2.1.220

Full-chain review of `core/workflow`, `core/workflow-runtime`,
`core/tools/tools/workflow*.rs` and their consumers against
`analyze/42_workflow` (2.1.220), source-verified against
`claude-code-bomb/versions/2.1.220/extract/cli_inner_pretty.js` where the
analysis was thin or wrong.

**Verdict: structurally aligned.** The subsystem coco-rs ships is the same
subsystem — same DSL, same sandbox posture, same caps, same launch/journal/
resume shape.

Two passes are recorded here. The first found **three genuine correctness bugs**
(all in the resume/nesting/state-model area) plus three robustness or policy
gaps, and listed eight remaining differences (§2). The second worked that list:
five implemented, one retracted with a reason, two left open (§4, §5). What
remains different on purpose is §3.

---

## 1. What was already right

Verified present and faithful, so nobody re-derives them as gaps:

| Mechanism | CC anchor | coco-rs |
|---|---|---|
| Concurrency width `min(16, max(2, cpus−2))`, FIFO, per run | `zWy` `:387140` | `workflow_local_concurrency` + `Semaphore` |
| Child `workflow()` shares the parent's semaphore | `:386901` | same host Arc ⇒ same semaphore |
| 1000-call lifetime agent cap | `WSd` `:388110` | `WORKFLOW_AGENT_CAP` |
| Token budget checked at *schedule* time; in-flight agents finish | `:387195-387201` | `budget_exhausted()` pre-call gate |
| `parallel()` = `allSettled` barrier, rejection → `null`, never rejects | `:388016-388048` | JS combinator, same |
| `pipeline()` = no barrier, `(prev, item, index)`, `null` short-circuits | `:388059-388068` | JS combinator, same |
| One-level `workflow()` nesting, rejecting stub in the child | `DSd` `:386905` | depth-aware global |
| `4096`-item array cap | `Aft` `:385841` | `WORKFLOW_ARRAY_CAP` |
| Determinism shim (`Math.random`, `Date.now`, bare/argless `Date`, the `RealDate.prototype.constructor` re-point + freeze) | `UWy` `:386390` | `sandbox.rs`, ported verbatim |
| Static AST determinism pre-check as the *ergonomic* layer, shim as the guarantee | `Uxo` `:386412` | `coco_workflow::meta` |
| `meta` = first statement, pure literal, `__proto__`/`constructor`/`prototype` rejected | `$H` `:275599` | `meta.rs` |
| 512 KiB source cap enforced by read-limit, not stat-then-read | `o1` `:162044` | `read_capped(limit+1)` |
| Named lookup matches parsed `meta.name`, never builds a path | `Dsn` `:388331` | `resolve_named_workflow` |
| `scriptPath` outranks `name` outranks `script`; inline body overrides, path stays provenance | `yEd` `:389188` | `resolve_workflow_source` |
| Permission rule key is `scriptPath ? undefined : name`; the dialog shows resolved script | `:389434`, `:389444` | `workflow_rule_key` + preview |
| Per-agent stall watchdog + retry ladder | `r6y`/`USd` `:388131` | `WORKFLOW_STALL_MS_DEFAULT` / `_RETRY` |
| 30 s bounds a **sync slice**, not the run | `Bxo` `:386383` | `WORKFLOW_SYNC_EVAL_BUDGET` |
| `agent({schema})` forces StructuredOutput | `:387454` | `output_schema` on the spawn |
| Exactly-once completion notification | `pBe` `:301459` | `mark_notified_once` |
| Journal: append-only JSONL, per-line tolerance, `null` results never cached, result awaited before the value reaches the script | `JPs` `:387081` | `WorkflowJournal` |

---

## 2. Fixed in this pass

### 2.1 Resume replayed one recorded value for every repeated call — P0

`AgentCacheKey` hashed `(phase_title, prompt, canonical_opts)`. CC hashes
`(previousKey, prompt, canonicalOpts)` — a chain, so key *n* is unique to
position *n* (`FSd` `:387077`).

Two defects fell out of the mis-port:

1. **Repeated identical prompts collapsed onto one key.** Every
   `loop-until-count` / `loop-until-dry` script the tool prose teaches, and
   every `parallel()` fan-out over one prompt, records exactly one journal
   entry. On resume all N calls hit it and replay the same value.
2. **`phase` was in the key.** CC excludes it deliberately — *"grouping only;
   regrouping the progress tree should not re-run agents"* — so coco re-spawned
   the whole run whenever a phase was renamed.

**Fix:** the key is now `(call_index, prompt, canonical_opts)` where
`call_index` is the run-global `agent()` ordinal. This gives the same
longest-unchanged-prefix guarantee as the chain (editing, inserting or deleting
a call shifts every later ordinal, so the first change misses and the
divergence latch re-runs the tail) without carrying chain state across the
nesting boundary. `JOURNAL_KEY_VERSION` bumped `wfj1` → `wfj2`.

Lookup also became **synchronous** — the journal hydrates once at launch, so a
probe is a map read. That puts the replay decision and the divergence latch
back in `agent()` call order even when `parallel()` fired the calls together.

### 2.2 Nested `workflow()` reset the run's counters — P0

`install_globals` allocated the agent ordinal, the phase counter and the
divergence flag as context-local `Rc<Cell<…>>`. A nested `workflow()` re-enters
`WorkflowEngine::run` with a fresh context, so a child restarted all three:
the 1000-agent lifetime cap was bypassable by nesting, child progress rows
collided with the parent's on index, and the replay cursor restarted.

The crate doc already *claimed* these were shared ("same host Arc ⇒ shared …
agent counter") — the code did not.

**Fix:** new `WorkflowRunState` (`run_state.rs`), an `Arc` the host parks on
itself and forwards to the child engine. `WorkflowEngine::run` now takes a
`WorkflowRun` params struct carrying `host` **and** `state`, and the trait doc
spells out that implementors must forward both.

### 2.3 `phase()` never interned; agents were never grouped — P1

`phase()` incremented a counter, so `phase('Scan')` twice made two groups;
indices were 0-based (CC reserves 0 for the ungrouped fallback);
`meta.phases[].title` was parsed and thrown away instead of seeding the
skeleton; and `agent_event` hardcoded `phase_index: None`, so no agent row ever
carried a group.

**Fix:** `WorkflowRunState::resolve_phase` interns by exact title (1-based),
`meta.phases` titles are pre-interned at launch, `phase()` publishes the group
node only on first sight, and `agent({phase})` resolves through the same table
and stamps `phase_index` on every agent row.

### 2.4 Progress state was unbounded and quadratic — P1

`TaskManager::push_workflow_progress` appended every delta to a `Vec` and
cloned the whole vec into a `task/progress` event per delta. CC's reducer
(`qPs` `:386523`) upserts agent/phase nodes by `(kind, index)` and trims logs at
`2 × 500` down to `500`.

Without the upsert: one `agent()` call renders as two rows (start + done), any
count derived from the array is wrong, and a 1000-agent run allocates ~2000
nodes copied ~2000 times. Without the trim, a `log()` loop grows forever.

**Fix:** `tasks::workflow_progress::apply_workflow_progress` (pure, tested)
does both. The TUI's `merge_workflow_progress` was concatenating when the
incoming array was not a prefix-extension of the held one — which upsert makes
the common case, doubling the array — so it now adopts the snapshot and only
falls back on an empty payload. Agent nodes are stamped with `last_progress_at`
so the panel's one-line summary tracks the freshest agent rather than the
last-inserted node.

### 2.5 Concurrent `git worktree add` — P1

`parallel()` of N `isolation: 'worktree'` agents ran N concurrent
`git worktree add` against one repo. `git` takes a repository lock and mutates
`.git/worktrees/`; the losers fail their spawn outright. CC serialises at width
1 (`I = AB(1, MDt)` `:387168`). The call was also a blocking subprocess on an
async worker thread.

**Fix:** `AgentWorktreeManager::create_for_serialized` — per-root
`tokio::sync::Mutex` + `spawn_blocking`. It is now the entry point for async
callers; the raw sync `create_for` is documented as blocking.

### 2.6 `agent({agentType})` was neither validated nor permission-gated — P1

`definition_for_opts` synthesised a bare definition for any unknown agentType,
so a typo silently ran a *different* agent with no signal to script or user;
and it never consulted `Agent(<type>)` deny rules, so `deny: ["Agent(x)"]` was
sidesteppable by asking a workflow to spawn `x`. CC filters the registry
through the Agent tool's own rule surface (`:387428-387451`) precisely because
a workflow script is model-authored code approved once.

**Fix:** explicit types resolve against the live catalog and the shared
`find_agent_deny_rule`; denied reports the rule and its source, missing lists
the *permitted* alternatives only (naming denied ones would leak restricted
agents). No `agentType` still resolves to general-purpose.

### 2.7 The main agent could not tell "found nothing" from "returned nothing" — P1

The completion notification carried `<result>` and nothing else. 2.1.220 added
three coordinated edits aimed at one failure mode — a workflow completes,
returns `[]`, and the model concludes the codebase has no instances of whatever
it was looking for (`<diagnostics>` `:386699`, the `agents_empty_result` census
`:386738`, the tool-prose sentence `:389101`).

**Fix:** a `<diagnostics>` block on both terminal paths, outside `<result>` so
it can never be read as the answer. Completed runs get the `journal.jsonl`
pointer, the "Read this before diagnosing" instruction, and the re-run hint;
failed/stopped runs get the literal `Workflow({resumeFromRunId})` call. Both
carry the census (`agents_done` / `_error` / `_skipped` / `_empty_result`) with
CC's deliberately narrow empty-shape test (`[]`, `{}`, `{"k": []}` — not
`{"count": 0}`).

---

## 3. Known divergences — deliberate

| CC behaviour | coco-rs | Why |
|---|---|---|
| `isolation: 'remote'` runner (~110 lines, width-50 semaphore) | rejected with a message | Dead in **both** CC bundles (`:387393` throws unconditionally). Porting shipped-but-fenced code is pure cost. |
| Adopt path + `scriptSha256` content pin (`sEd` `:388865`) | absent | coco resume is same-session and always goes through `checkPermissions`, so the human is in the loop. The pin exists in CC precisely because adopt has no dialog. Revisit **with** the daemon-worker lifecycle, not before. |
| Server-authored `workflow_launch` carrier + `/__remote-workflow` | absent | Needs the CCR transport coco doesn't ship. |
| Workflow subagents denied `Agent` outright (`eMs.disallowedTools`) | depth-gated (`< 5`) | coco's deliberate depth model; `Workflow` itself *is* denied, so the recursion guard holds. |
| Abort returns a forever-pending promise so no `catch` can observe it | `CancellationToken` ends the run | coco's is stricter, not weaker. |
| `meta.phases[].model` | parsed, unused | Unused in CC too (`:275733`, no consumer) — matching the inertness is correct. |

---

## 4. Closed in the follow-up pass

The second pass, in the order the first one ranked them.

### 4.1 `workflowSizeGuideline` — the window's headline governance feature

Ported whole, including the part that makes it interesting. A four-value enum
(`unrestricted | small | medium | large`, caps `5 / 15 / 50`) renders to one
English sentence appended to the Workflow tool's description. Nothing enforces
it — the caps only ever appear inside a template string, in CC as in coco.

Two details carried over because they are the design, not decoration:

- **The default and the explicit case are worded differently.** An unset
  guideline says *"This session has the **default** …"* and tells the model the
  user can change it in `/config`; an explicitly chosen one drops that clause.
  The escape hatch is a licence to argue with the constraint — useful when
  nobody chose anything, an invitation to override a decision the user already
  made when they did.
- **The value is pinned per session, not read live.** The description sits in
  the request's largest cacheable prefix; a value that tracked settings reload
  would re-bill the whole tool block on every edit. `ToolAppState.workflow_size`
  holds the pin *and*, separately, the last value announced to the model — a
  mid-session change travels as a `workflow_size_guideline_change` reminder,
  which is message-level and invalidates nothing.

coco's precedence chain is one store shorter than CC's: there is no mutable
`.claude.json` beside settings.json, so `settings → default` replaces
`settings → /config → default`. CC's documented failure mode (garbage in the
settings file hides the `/config` row while the `/config` value stays in force)
has no analogue here.

### 4.2 `agent({schema})` size bound

Placed at `ToolInputSchema::from_value` rather than in the workflow host, which
is where CC's `fty` sits relative to *its* three consumers: MCP wire schemas,
`--json-schema`, and `agent({schema})` all compile through this one seam.

The bounds are `MAX_SCHEMA_NODES = 100_000` (CC parity) and
`MAX_SCHEMA_DEPTH = 128` — **not** CC's `1e4`. CC's depth is calibrated for a
JS engine's stack; `jsonschema`'s compiler recurses on ours, and 10_000 frames
would overflow the guard's own reason for existing. 128 is `serde_json`'s
default recursion limit, so any schema that arrived as JSON text was already
held to it and nothing legitimate narrows. The bound walk keeps an explicit
stack for the same reason.

`WorkflowRunHost::run_agent` compiles the schema before the dispatch screen, so
an invalid one fails the `agent()` call the script can see instead of surfacing
turns later as a subagent that never called StructuredOutput.

### 4.3 Queued vs running

`started_at`'s absence is the whole discriminator, and coco had a live consumer
for it (`WorkflowAgentStatusFilter::Queued` reads
`queued_at.is_some() && started_at.is_none()`) with no producer — the filter was
unreachable.

The engine now owns an `AgentRow` and hands the host a `WorkflowAgentStarted`
callback fired once its permit lands: only the host knows when the queue wait
ended, and only the engine should decide what a row looks like. Every frame
re-states both timestamps, because the reducer replaces a node wholesale — a
field dropped from the terminal frame is a field erased from the run's record.

### 4.4 Nested-workflow presentation

`WorkflowRun::child_group` (`▸ name`, `▸ name #2` on re-entry) is `Some` only
for a nested run, and its presence rewires three globals exactly as CC does:
`phase()` becomes a no-op, `log()` is prefixed, and `agent()` overrides
`opts.phase`. The contract being protected is substitutability — a script must
produce the same progress tree whether launched directly or invoked by
`workflow()`.

### 4.5 Progress batching

The fold stays unconditional (the row is always current); only publishing is
rate-limited, at `WORKFLOW_PROGRESS_COALESCE_MS = 16`. Every frame carries the
whole array, so one frame per delta is quadratic in the delta count — a `log()`
loop or a wide fan-out pays it in full. Suppressed deltas ride a single trailing
flush that re-reads the row, so it carries everything accumulated rather than
just the delta that scheduled it, and `transition_terminal` settles any pending
frame before `TaskCompleted`.

### 4.6 `ultracode` on non-human input — retracted, not deferred

coco has no signal for "typed by a human" distinct from "arrived through the
SDK". `MessageOrigin` separates user input from system-injected and slash
commands, not terminal input from a relayed webhook; `is_non_interactive` is a
print-mode flag, and gating on it would silently disable the keyword for every
legitimate SDK user. The analysis explicitly did not trace CC's own
`isHumanTypedPrompt` propagation chain (it is `40_system_prompt`'s), so a port
would be guesswork with a real regression on the other side. This needs the
prompt-origin chain first; it is not a workflow-side change.

---

## 5. Still open

Both need something outside the workflow subsystem before they are worth doing.

1. **Name-only lockdown** — restricts a session to bundled named workflows by
   rejecting `script` / `scriptPath` / `resumeFromRunId` (CC's
   `CLAUDE_WORKFLOW_NAME_ONLY`, `:386782`). If ported it is
   `COCO_WORKFLOW_NAME_ONLY` as a `coco_config::EnvKey` variant — every
   coco-owned env var carries the `COCO_` prefix and is read through `EnvKey`,
   never `std::env::var` at the use site. Worth having once coco has a
   managed-policy story to hang it on; a lockdown nobody can deploy centrally
   is a flag, not a control.
2. **`ultracode` human-origin gate** — see §4.6. Blocked on a prompt-origin
   signal coco does not have.

Plus the deliberate divergences in §3, and the **hand-off review** half of the
auto-mode work (`tin`, runtime_core §7.4): advisory, never blocks, and absent on
all three of coco's subagent boundaries — a product decision, not a gap.

## 6. Where the analysis was wrong or thin

Recorded so the next pass does not re-derive them:

- `README.md` §1's opening claim — *"this window contains no new workflow
  capability"* — is retracted by its own §6. The changelog covers the
  governance and plumbing work only; the capability work went unannounced.
- The `workflow_agent` node's `isolation: "remote"` and `remoteSessionId`
  fields are unreachable in both bundles. Do not model them.
- `kvn` (`:651236`) is `return null`, so the over-size warning's `phase` field
  is permanently null while both its arguments are computed on every render.
- The log-trim reducer reads like the `.198` fix and is byte-identical
  carryover; the actual fix is in the publisher.
