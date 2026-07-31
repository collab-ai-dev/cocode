# Auto-mode subagent screening — analysis and remediation design

Follow-up to [workflow-alignment-2.1.220.md](workflow-alignment-2.1.220.md) §4.1.
Every CC claim below was re-read in
`claude-code-bomb/versions/2.1.220/extract/cli_inner_pretty.js`, not taken from
the analysis prose.

---

## 0. Correction to the earlier write-up

The previous summary described a single "hand-off classifier" gap and said it
"fails closed on an unclassifiable schema". That conflated **two distinct
mechanisms** with different scopes, different failure modes, and very different
severities. The corrected reading:

| | Dispatch screen | Hand-off review |
|---|---|---|
| CC symbol | `j` (inner, `:387224-387287`) → `vpd` (`:345775-345820`) | `tin` (`:345816-345875`) |
| Runs | **before** a subagent spawns | **after** a subagent finishes |
| Effect | **blocks** the dispatch — `agent()` resolves to `null` | **advisory** — prepends `SECURITY WARNING: …` to the result |
| Classifier unavailable | fails **open** (dispatch proceeds) | fails **open** (different warning) |
| Schema too large / unserialisable | fails **closed** (`:387248`, `:387252`) | n/a |
| CC call sites | **1** — workflow only (`:387424`, `:387912`) | **3** — sync Agent, background agent, workflow |

"Fails closed" belongs to the *dispatch* screen's schema path only. The hand-off
review never blocks anything.

---

## 1. Is the gap workflow-only?

**Two different answers, and the distinction is the whole point.**

### 1.1 Dispatch screen — YES, workflow-only, and it is the real finding

`vpd` fabricates a synthetic **`Agent` tool call** and classifies it:

```javascript
// :345795 — qo === "Agent" (:162358)
c = await E$t(n, Xpr(qo, { prompt: l, subagent_type: t }), [BNy, ...o], i, s, …)
```

That is the mechanism stating its own purpose: a workflow's `agent()` is
screened *as if it were* the `Agent` tool call it stands in for.

It has to be, because of an asymmetry that exists identically in coco-rs:

| Dispatch path | Reaches the auto-mode classifier? |
|---|---|
| Model emits `Agent(prompt, subagent_type)` | **Yes** — it is a tool call; `tool_call_preparer.rs:1031` |
| Model emits `Task(...)` | Yes — same pipeline |
| Workflow script calls `agent(prompt)` | **No** — a script call into a host fn; never a tool call |

Verified in coco-rs:

- `Agent` and `Workflow` are **not** in `SAFE_TOOLS`
  (`core/permissions/src/classifier.rs:160-191`), so both are classified when the
  evaluator returns `Ask`.
- `WorkflowRunHost::run_agent` → `build_request` → `AgentHandle::spawn_agent`.
  No permission evaluation, no classifier, anywhere on that path.
- The classifier gate is `matches!(decision, PermissionDecision::Ask { .. })`
  (`tool_call_preparer.rs:498`). An explicit **allow rule short-circuits it** —
  so a `Workflow(my-build)` rule means the script is never classified either.

So in auto mode: approve `Workflow` once (or hold a `Workflow(name)` allow rule)
and every `agent()` the script issues thereafter is an unscreened subagent
dispatch, with prompts the script computes at runtime from `args`, from files, or
from a previous agent's output. The equivalent `Agent` tool call would have been
screened.

### 1.2 Hand-off review — originally absent, now implemented

CC's three `tin` call sites:

| Site | Path | What it does with the warning |
|---|---|---|
| `:346180` | background / async agent completion | prepends to `finalMessage` in the task notification |
| `:387866` | workflow `agent()` | prepends to `result.text`; for `{schema}` agents routes to `failures` + a `workflow_log` line instead, because their text channel is not their return channel |
| `:399158` | **sync `Agent` tool** | prepends a text block to the tool result content |

The first review found zero hand-off call sites. That finding is now closed:
`coordinator/src/agent_handle/handoff.rs` owns the two-stage classifier, and
`coordinator/src/agent_handle/spawn.rs` invokes it for synchronous, background,
and workflow-backed completion paths. It remains cross-cutting rather than a
workflow-only concern.

---

## 2. Cross-validation — do these genuinely matter?

Arguing against my own findings, because a screen that only sounds important is
worse than none.

**Counter-argument 1: "The subagent's own tool calls are still classified, so
nothing dangerous executes."** *Largely correct, and it caps the severity.*
`resolve_subagent_mode` (`mode_transition.rs:77-90`) puts `Auto` in the trust
set, so a subagent of an auto-mode parent runs in `Auto` and every Bash / Write /
Edit it issues goes through `can_use_tool_in_auto_mode` with its own context.
The dangerous *execution* remains gated.

What the dispatch screen adds is therefore **defense in depth, not the only
line of defense**:

- it judges the **whole requested task** in the **parent's** context, where the
  per-call classifier sees one command in the subagent's narrow context;
- it refuses **earlier and cheaper** — before a spawn, its tokens, and its tool
  calls;
- it closes a **parity hole**: identical intent is screened via `Agent` and
  unscreened via `Workflow`. Parity holes are what get found and used, whatever
  the residual risk of any single one.

**Counter-argument 2: "CC's own dispatch screen fails open when the classifier
is unavailable, so it is best-effort anyway."** *Correct* (`if (!Ce) return !1`,
`:387268`). It is a screen, not a gate. That is an argument about how loudly to
describe it — not about whether coco should have it.

**Counter-argument 3: "The Workflow tool call itself is classified, so the
script text was already reviewed."** *True but weaker than it looks.* It
classifies the **script source**, not the **prompts the script computes**; and an
allow rule skips it entirely (§1.1).

**Counter-argument 4 (against the hand-off review): "It never blocks, so its
absence cannot cause harm."** *Correct.* It is a missing warning channel, not a
missing gate. It also costs a classifier round-trip per subagent completion —
material on a 50-agent fan-out, which is why CC gates it behind `Cpd(…)`
returning a non-empty transcript projection (`:345862`).

### Verdict

| Finding | Genuine? | Severity | Scope |
|---|---|---|---|
| Workflow `agent()` bypasses the auto-mode dispatch screen | **Yes** | Medium — defense-in-depth + parity hole; per-call classification still gates execution | **Workflow only** |
| Hand-off review on all subagent boundaries | **Resolved** | Advisory, fails open, never blocks | All three paths |

---

## 3. Remediation design

### 3.1 Options considered

**A. A screening handle on `ToolUseContext`, implemented in `app/query`.**
`core/tools` presents the dispatch; the implementor rebuilds the equivalent
`Agent` tool call and runs the **same** `can_use_tool_in_auto_mode` the real
Agent call would hit.
*Chosen.* Correct layering, no new dependency edges, and it is the pattern this
codebase already uses for exactly this problem — `agent`, `task_handle`,
`side_query`, `mcp`, `lsp`, `goal` are all `Option<Handle>` on
`ToolContextFactory` with a `NoOp*` default. Sharing the decision function is
what structurally prevents the two paths from drifting apart again.

**B. `core/tools` calls `classify_yolo_action` directly via `ctx.side_query`.**
*Rejected.* It would have to re-derive `AutoModeContext`, path-safety immunity,
the denial tracker and the headless fail-closed rule that
`can_use_tool_in_auto_mode` owns — or skip them, reintroducing the very
divergence being fixed. It also puts permission policy in `core/tools`, the
wrong layer.

**C. Screen inside `AgentHandle::spawn_agent`.**
*Rejected.* Covers every spawn path uniformly, but double-screens the Agent tool
(already classified upstream) — extra latency and tokens on the hot path — and
conflates spawn mechanics with permission policy.

### 3.2 Shape

```rust
// core/tool-runtime/src/subagent_screen.rs
/// A subagent dispatch that did not arrive as a model tool call.
pub struct SubagentDispatch<'a> {
    pub prompt: &'a str,
    pub subagent_type: Option<&'a str>,
    pub output_schema: Option<&'a serde_json::Value>,
    /// The dispatching turn's permission context. The screen reads its mode
    /// and rules, exactly as the Agent tool's own check does.
    pub permission_context: &'a coco_types::ToolPermissionContext,
    /// The dispatching agent's transcript, so the classifier judges the
    /// request in the context that produced it.
    pub messages: &'a [Arc<coco_messages::Message>],
}

pub enum SubagentDispatchVerdict {
    Allow,
    Block { reason: String },
}

#[async_trait]
pub trait SubagentDispatchScreen: Send + Sync {
    async fn screen(&self, dispatch: SubagentDispatch<'_>) -> SubagentDispatchVerdict;
}
```

`NoOpSubagentDispatchScreen` returns `Allow` — consistent with every other
handle here, and defensible because the screen is a second layer over per-call
classification, which still runs inside the subagent.

**Where each policy decision lives:**

| Decision | Home | Why |
|---|---|---|
| `mode != Auto` ⇒ allow | the impl | keeps every policy decision on one side of the seam |
| schema → `[output schema]` text, 4 KiB cap, fail **closed** | the impl | it is about what the *classifier prompt* may contain |
| blocked ⇒ `agent()` yields `null`, not a throw | the engine | CC returns `null` from `K` (`:387427`); a refused dispatch is a dropped slot, not a script error |

### 3.3 Required engine change — `run_agent` cannot express "refused"

`WorkflowHost::run_agent -> Result<WorkflowAgentResult, String>` has only
"value" and "error", and `Err` becomes a rejected promise. A blocked dispatch
must resolve to `null` instead, so:

```rust
pub enum WorkflowAgentOutcome {
    /// The subagent ran and produced a value.
    Completed(WorkflowAgentResult),
    /// Refused before any spawn — auto-mode screen, or a user skip.
    /// `agent()` resolves to `null` rather than rejecting: a refused slot is a
    /// dropped slot, not a script error.
    Refused { reason: String },
}
```

This is worth doing for its own sake — it is also the missing home for CC's
`skipWorkflowAgent` semantics (`Vfr` `:386649`), where a user-skipped agent
likewise returns `null`.

`WorkflowProgressEvent::WorkflowAgent` gains `blocked: bool` alongside the
existing `skipped: bool`; CC keeps them distinct (`:387286` vs `:387693`) and so
should the completion census, which today folds a refusal into `agents_error`.

### 3.4 Wiring cost

One field + one builder method + one `unwrap_or_else` on `ToolContextFactory`
(`app/query/src/tool_context.rs`), the impl constructed where the engine already
holds `auto_mode_rules` / `model_runtimes` / `usage_accounting`, and
`WorkflowSpawnContext` capturing `ctx.messages` at launch (it already captures
`permission_context`).

### 3.5 Hand-off review — implemented in the coordinator

The follow-up implementation lives at the completion boundary rather than on
`SubagentDispatchScreen`: `coordinator/src/agent_handle/handoff.rs` projects the
transcript, runs both classifier stages, and returns the original result with an
advisory warning when required. Keeping it in the coordinator gives all three
completion paths one policy and avoids coupling post-run review to the
pre-dispatch tool context.

---

## 4. Status

**§3.1–3.5 are implemented.** The dispatch screen covers workflow calls that
bypass the normal tool pipeline; the coordinator hand-off classifier covers
all completion paths.

Shipped:

- `core/tool-runtime/src/subagent_screen.rs` — the seam, with
  `NoOpSubagentDispatchScreen` as the unwired default.
- `app/query/src/subagent_screen_impl.rs` — `AutoModeSubagentScreen`, which
  rebuilds the `Agent` tool call and calls `try_classify_in_auto_mode`. Holds
  a dispatch-scoped `DenialTracker`, deliberately not the turn's: a detached
  run has nobody to escalate an interactive prompt to.
- `WorkflowAgentOutcome::{Completed, Refused}` — a refused dispatch resolves to
  `null` instead of rejecting.
- `WorkflowProgressEvent::WorkflowAgent.blocked`, counted as `agents_blocked`
  in the completion census, separate from `agents_error`.

One behaviour worth recording, because it looks wrong and is not: the `Start`
row is emitted before the host is consulted, so a refusal arrives as a *second*
frame on the same index. CC does the same (`at()` at `:387394` precedes the
screen inside `K` at `:387424`), and the index-keyed progress reducer collapses
the pair onto one node whose final state is the refusal.
