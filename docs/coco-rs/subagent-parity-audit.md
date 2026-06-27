# Subagent Parity Gap Audit (TS ↔ coco-rs)

Date: 2026-05-15
Scope: `coco_subagent`, `AgentTool`, `SwarmAgentHandle` (coordinator), and the
seams in `app/query`, `app/cli`, `core/tools`, `memory`, `hooks`.
Reference: `tools/AgentTool/**`, `utils/swarm/**`. TS file paths are relative
to the TS project's `src/` directory.

This document is an **adversarial parity audit**, not a refactor plan. It
catalogs every functional / behavioral divergence I could verify against
TS, ordered by user-visible impact. Each row is anchored to a `file:line`
on both sides so future work can be checked without re-investigating.

---

## TL;DR

The Rust subagent crate is **architecturally complete and substantially
TS-aligned**. Consumer wiring through `AgentTool → SwarmAgentHandle →
AgentQueryEngine` is real (no legacy `agent_spawn.rs` / `agent_advanced.rs`
left), the 6-agent built-in roster matches, the 5-layer tool filter
pipeline matches at the constant level (with two edge-case divergences),
and frontmatter `memory:`, `hooks:`, MCP `required_mcp_servers`,
`memory_scope` injection, fork mode, coordinator mode, and SubagentStop
firing are all genuinely implemented (verified by reading the source, not
just docs).

The remaining gaps fall into three buckets:

| Bucket | Count | What's there |
|---|---|---|
| **P0 — semantic-correctness bugs** | 3 | Things that silently produce wrong behavior under load (cache busting, hung syncs, wildcard auto-memory) |
| **P1 — visible parity gaps** | 6 | Behaviors TS supports that Rust does not yet (`Workflow` block, ant-build `Agent` re-admit, `mcpServers` inline form, 2-level walk, plan-mode child swap, dynamic input enum) |
| **P2 — quality-of-implementation** | 4 | Defendable as-designed but worth noting (built-in prompt paraphrase, `extra_allow_list` carry-through, handoff-classifier idle, summary 30s timer behavior) |

Two earlier audit claims I had to drop after direct verification:
**memory tool auto-inject is implemented** (`definition_store.rs:300-304,440-455`),
and **consumer wiring is real** (`session_runtime.rs:1163-1211 → engine.agent_catalog → AgentTool::prompt`).

---

## Methodology

For each candidate gap:
1. Read the TS reference site (file:line, full function body).
2. Read the Rust equivalent (every relevant file, not just headers).
3. If the Rust path runs the behavior, confirm it actually fires at the
   right seam (load vs spawn vs render).
4. Classify by severity using:
   - **Reachability** — what fraction of subagent spawns hit this?
   - **Failure mode** — silent vs visible vs crash?
   - **TS-parity contract** — is this a documented invariant or a quiet detail?

---

## P0 — Semantic-Correctness Bugs

### P0-1. Fork-mode model is not pinned to parent → prompt-cache invalidation

**Severity rationale.** Fork mode's *entire purpose* is to share the parent's
prompt cache: TS sets `model: undefined` explicitly so the resolver falls
back to `mainLoopModel` (`AgentTool.tsx:610`). If a fork spawn arrives with
`model: "sonnet"` (model-supplied, user-supplied, or copy-pasted from a
non-fork example), the cache key changes and the cache benefit
*silently* disappears. Bills go up, latency goes up, nothing logs.

**TS contract.**
`tools/AgentTool/AgentTool.tsx:608-612` — fork branch passes
`model: undefined` to `runAgent` regardless of input.

**Rust state.**
`core/tools/src/tools/agent/agent_tool.rs:422-459` builds
`SpawnMode::Fork { rendered_system_prompt, parent_messages,
inherit_tool_pool }` but does **not** strip `request.model` for the fork
branch. The model field flows through to
`AgentSpawnRequest.model` (line 535-538) → `selection.model`
(`spawn.rs:643-647`) → `AgentQueryConfig.model` (`spawn.rs:770-773`)
unchanged. The prompt renderer only *advises* the LLM not to set `model`
on a fork; nothing in code enforces it.

**Fix sketch.** In `agent_tool.rs::execute`, after deciding
`SpawnMode::Fork`, force `request_model = None`. Equivalent enforcement
inside `spawn_resolution::resolve_subagent_selection` would also work
but couples the resolver to fork awareness, which is worse.

**Verification.** Add a test in `coco-subagent::fork.test.rs` that asserts
`resolve_subagent_selection(Some("sonnet"), def, …)` returns
`SubagentSelection { model: None, .. }` when the call is fork-mode. Today
it returns `Some("sonnet")` — that's the bug.

---

### P0-2. Sync subagent cancellation does not propagate from parent

**Severity rationale.** When the user hits Ctrl+C / Esc during a parent
turn that has a sync `AgentTool` call in flight, the parent's
`QueryEngine` returns from `await` but the child engine keeps running
until it naturally finishes its turn budget. There's no foreground way to
abort it. The background path is fine — `tokio::spawn` + its own internal
`CancellationToken` (`spawn.rs:1067`) — but background spawns are
explicitly fire-and-forget, so they shouldn't inherit parent cancel
anyway. The sync case is the bug.

**TS contract.**
`tools/AgentTool/runAgent.ts:520-528` — the sync path computes
`agentAbortController = override?.abortController ??
toolUseContext.abortController` so the parent's signal is the default.
`AgentTool.tsx:445` threads `signal: toolUseContext.abortController.signal`
into the inner `query` call.

**Rust state.**
`core/tool-runtime/src/agent_handle.rs:122-297` — `AgentSpawnRequest`
has no `cancel` / `CancellationToken` field at all.
`core/tool-runtime/src/agent_query.rs:28-130` — `AgentQueryConfig`
similarly lacks a cancel field.
`coordinator/src/agent_handle/spawn.rs:903` —
`engine.execute_query(&effective_prompt, query_config).await` is awaited
without any way to interrupt it. The Tool execution layer *does* honor
`ctx.cancel` (`core/tool-runtime/src/execution.rs:190,310-318`) but the
inner subagent's query loop is a different code path that never sees it.

**Fix sketch.**
1. Add `pub cancel: Option<CancellationToken>` (or non-`Option`,
   defaulting to a fresh token) to `AgentSpawnRequest`.
2. At the `AgentTool::execute` boundary (`agent_tool.rs:~625`), set
   `cancel: Some(ctx.cancel.child_token())` so the child token is
   cancelled if the parent's tool-execution context is cancelled.
3. In `SwarmAgentHandle::spawn_subagent` sync branch, race
   `engine.execute_query(...)` against `cancel.cancelled()` with
   `tokio::select!` (the bg path already does this at
   `spawn.rs:1302`).
4. Background path leaves its own internal token in place — the parent's
   cancel does *not* automatically tear down a fire-and-forget bg agent
   (TS parity).

**Verification.** Hard to unit-test the engine race, but a coordinator
integration test can spawn a sync subagent that loops in a tool, cancel
the request, and assert the spawn returns within a small window.

---

### P0-3. Auto-memory injection skips wildcard agents → memory writes silently denied

**Severity rationale.** Subtle. The current code only injects Read/Edit/Write
into `allowed_tools` when the agent declares a *non-empty* allow-list
(`definition_store.rs:445-448`). Comment claims this is TS parity because
"wildcard already sees every tool". That's true for the *registry-visible*
set, but `allowed_tools` is *also* the source that
`AgentToolFilter::plan()` consults (`filter.rs:118-195`) and that the
prompt renderer uses to build the "Allowed tools: …" string the model
sees. For a wildcard agent with `memory: project`, the prompt currently
says "All tools" (the wildcard branch) which is functionally correct, so
this is borderline. **But** if any future filter layer (or
`tool_overrides` from the parent) narrows the set to exclude Write, the
agent will silently lose memory-write capability and the user will see
"my custom agent stopped saving memories" with no log line.

**TS contract.**
`tools/AgentTool/loadAgentsDir.ts:455-462` — TS guards on
`tools !== undefined` (i.e. the user supplied any list), and injects
Read/Write/Edit. Wildcard *with `tools: ["*"]` explicitly written* still
takes the injection — TS treats `["*"]` as a non-undefined value and
includes it. coco-rs collapses wildcard and "tools: ['*']" into the same
empty `allowed_tools` representation, which loses that distinction.

**Fix sketch.** Add an explicit `wildcard_tools: bool` (or
`tools: Option<Vec<String>>`) on `AgentDefinition` so we can tell
"user wrote `tools: ['*']`" apart from "user wrote nothing." Inject
memory tools when `memory_scope.is_some() && tools.is_some()` (matches
TS's `tools !== undefined`).

**Verification.** Add a test in `definition_store.test.rs` parsing
`memory: project\ntools: ["*"]` and asserting `allowed_tools` contains
Read/Edit/Write. Today it doesn't.

---

## P1 — Visible Parity Gaps

### P1-1. `ALL_AGENT_DISALLOWED_TOOLS` unconditionally blocks `Agent` (ant build path lost)

**TS** (`constants/tools.ts:36-44`):
```ts
new Set([
  TASK_OUTPUT_TOOL_NAME,
  EXIT_PLAN_MODE_V2_TOOL_NAME,
  ENTER_PLAN_MODE_TOOL_NAME,
  ...(process.env.USER_TYPE === 'ant' ? [] : [AGENT_TOOL_NAME]),
  ASK_USER_QUESTION_TOOL_NAME,
  TASK_STOP_TOOL_NAME,
  ...(feature('WORKFLOW_SCRIPTS') ? [WORKFLOW_TOOL_NAME] : []),
])
```

**Rust** (`core/subagent/src/filter.rs:13-21`):
```rust
pub const ALL_AGENT_DISALLOWED_TOOLS: &[&str] = &[
    ToolName::TaskOutput.as_str(),
    ToolName::ExitPlanMode.as_str(),
    ToolName::EnterPlanMode.as_str(),
    ToolName::Agent.as_str(),       // unconditional
    ToolName::AskUserQuestion.as_str(),
    ToolName::TaskStop.as_str(),
];
```

`Agent` is always in the deny list. The filter does *partially* re-admit it
for teammates (`filter.rs:~152`), but a sync subagent in an ant-style
build cannot nest another `Agent` call. For coco-rs's 3P scope this is
arguably correct (no internal ant build), but if `USER_TYPE=ant` ever
becomes a coco-rs concept the constant needs to flip via a function
parameter, not a `const`.

**Recommendation.** Either (a) make the constant a function
`fn all_agent_disallowed_tools(is_ant: bool, has_workflow: bool) ->
Vec<&'static str>` like TS does at module-init, or (b) document in the
crate that nested Agent calls are forbidden under all configurations and
delete the comment in `agent_tool.rs` mentioning ant.

---

### P1-2. `Workflow` tool not in the type system → can't be filtered when shipped

**TS** (`constants/tools.ts:44`):
```ts
...(feature('WORKFLOW_SCRIPTS') ? [WORKFLOW_TOOL_NAME] : []),
```

**Rust.** `ToolName` enum (in `coco-types`) has no `Workflow` variant.
The blocker isn't urgent — coco-rs doesn't ship `Workflow` today — but
**when it does**, the deny-list mechanism (currently a `&[&str]` const)
can't conditionally include it without a code change. Mirror TS by making
the const a builder once `Workflow` lands.

---

### P1-3. `mcpServers` inline form `{ name: { command, args, env } }` not parsed

**TS** (`tools/AgentTool/loadAgentsDir.ts`) accepts both:
```yaml
mcpServers: [github, linear]                          # string-ref
mcpServers: [{ slack: { command: "/path", args: [] } }]  # inline
```

**Rust** (`core/subagent/src/frontmatter.rs:182-201`) — only string-ref
parsed (acknowledged in `core/subagent/CLAUDE.md` "Known Phase-1 Gaps").

**Impact.** A user porting an inline-form agent.md from TS gets the agent
parsed (warning, not failure) but with `mcp_servers: []`, so
`required_mcp_servers` gating won't apply and the per-agent MCP server is
silently skipped. Visible to anyone who actually uses inline MCP servers
(rare in practice; common in the Anthropic builtin agents repo).

**Fix.** Extend `parse_value` to also accept `Value::Object` entries
inside the array; route to `mcp_servers` as a typed `McpServerConfig`
enum (string-ref vs inline). Then update
`coordinator/src/agent_handle/spawn.rs::initialize_per_agent_mcp` to
register inline servers via the existing dynamic-MCP path.

---

### P1-4. Definition store walks one directory level; TS walks two

**TS** uses `walkdir { max_depth: 2 }` to find agent `.md` files in
nested subdirs (e.g. `.claude/agents/refactor/explore.md`).

**Rust** (`core/subagent/src/definition_store.rs:39-92`'s
`sorted_md_paths`) does a single-level `read_dir`. Nested files are
invisible. Acknowledged in `core/subagent/CLAUDE.md`.

**Impact.** Users who organize agents into subfolders silently lose
those agents from the catalog. The CLI `/agents list` shows fewer items
than the filesystem has. Easy to miss because nothing errors.

**Fix.** Pull in `walkdir = "2"` (it's already a transitive dep of
`coco-skills`), replace `read_dir` with `WalkDir::new(dir).max_depth(2)`.
Apply the existing 1 MiB size cap inside the walker. Symlink loop
protection via existing `(dev, ino)` set (already on Unix
path).

---

### P1-5. `subagent_type` input is not enum-constrained → model can hallucinate types

**TS** dynamically rebuilds the tool's JSON Schema at session start
with `subagent_type` as a Zod enum of `getActiveAgents().map(a =>
a.agentType)`. The model literally cannot emit an unknown type — the
schema rejects it.

**Rust** (`core/tools/src/tools/agent/agent_tool.rs:input_schema`) leaves
`subagent_type` as `type: string` with prose examples. The prompt
*lists* the valid types, but the schema is permissive. If the model
hallucinates "Coder" the call reaches `execute()`, which falls through
to a hard-coded `general-purpose` default (`spawn.rs:507-510`) without
warning. Visible result: the wrong agent runs.

**Fix.** Rebuild the schema at turn boundary from the catalog snapshot
(coco-rs already rebuilds the prompt this way). Easier alternative: emit
an explicit `Err(...)` at the spawn boundary when `subagent_type` is set
but not in the catalog (today the code silently picks `general-purpose`).

---

### P1-6. Plan-mode subagents inherit `permission_mode` but not the `plan_role_client`

**TS** drives plan mode by swapping the client for the *active* loop. A
plan-mode subagent's parent has its client swapped at the leader level;
the subagent receives `permissionMode: "plan"` and is expected to use
the role's configured model.

**Rust** (`app/query/src/engine.rs:1056-1087`) swaps to
`self.plan_role_client` only for the leader's main loop. Subagents do
*not* get a `plan_role_client`; their model resolution goes through
`selection.model_role` → role table. This usually works for the built-in
`Plan` agent (it declares `model_role: Plan`), but for a *custom* agent
spawned with `mode: "plan"`, the child uses its definition's
`model_role` (probably `Subagent`), which doesn't route to the plan
model.

**Impact.** A custom agent invoked with `mode: "plan"` runs on the
wrong (cheaper, less-capable) model. The behavior is invisible — the
agent just performs slightly worse.

**Fix.** In `coordinator/src/agent_handle/spawn.rs`'s `query_config`
construction (line ~763), when `permission_mode == Some("plan")`, force
`model_role = Some(ModelRole::Plan)` so role resolution picks the plan
client.

---

## P2 — Quality-of-Implementation

### P2-1. Built-in `whenToUse` and `system_prompt` strings are paraphrases of TS

`core/subagent/CLAUDE.md` itself flags this:
> Built-in `whenToUse` strings are short paraphrases, not the verbatim TS
> strings from `built-in/*.ts`. The model-facing prompt list will read
> slightly differently from TS until the prompt renderers ship in Phase 2.

Same applies to `builtin_prompts.rs` — the system prompts for
GeneralPurpose / Explore / Plan / Verification / CocoGuide / StatusLine
are written in idiomatic Rust prose, not the literal TS strings. The
behavior difference is small (most builtin prompts are short), but
*evaluations comparing TS vs Rust on identical inputs will diverge* on
the agent's first-turn behavior.

**Recommendation.** Port the exact TS strings verbatim via `include_str!`
into `builtin_prompts.rs`. Low risk, high parity payoff. Document the TS
file (`built-in/<agent>.ts`) in a comment at each `include_str!` site so
future drift is visible.

### P2-2. `extra_allow_list` is coco-rs-only and unwired in the tool filter pipeline

`core/subagent/src/filter.rs:92` defines `extra_allow_list` on
`ToolFilterContext`, intended for slash-command tool intersection
(Phase 8 reservation). No call site populates it yet; the field is dead
weight today. Either wire it (skill-command path) or remove it until
needed.

### P2-3. Handoff classifier is implemented but not orchestrated by default

`core/subagent/src/handoff.rs` (the 2-stage safety classifier) is ready;
coordinator-side wiring exists. But the orchestration site that decides
*when* to run it is gated off (it's a paid LLM call). Result: the
classifier is dead code at runtime unless a feature flag flips. Either
ship the flag with a clear policy ("ant build runs classifier
post-spawn") or delete the orchestration shim until decision is made.

### P2-4. `AgentSummary` 30s timer fires only for background spawns

`spawn.rs:1148-1167` registers a periodic summary for the background
path. Sync spawns don't get one (they bubble back through tool output).
Matches TS roughly, but TS *also* emits per-turn summaries to the task
panel for sync agents — if the goal is feature parity with the TS UI,
sync summaries belong on the streaming-event path. As-is, this is a
minor UX divergence.

---

## Cross-Cutting Observations

### Observation 1 — Filter constants are 29/31 identical

Side-by-side audit (verified, not estimated):
- `ALL_AGENT_DISALLOWED_TOOLS`: 5/6 (missing `Workflow`, `Agent` gating
  divergence — see P1-1, P1-2)
- `CUSTOM_AGENT_DISALLOWED_TOOLS`: alias-to-`ALL_AGENT_DISALLOWED_TOOLS`
  on both sides ✓
- `ASYNC_AGENT_ALLOWED_TOOLS`: 16/16 ✓
- `IN_PROCESS_TEAMMATE_ALLOWED_TOOLS`: 8/8 ✓ (TS feature-gates
  CronCreate/Delete/List via `feature('AGENT_TRIGGERS')`; Rust enforces
  via `Tool::is_enabled` upstream — same outcome, different layer)

### Observation 2 — Multi-LLM story is genuinely transparent

The chain `request_model > definition.model > role-resolved` in
`spawn_resolution::resolve_subagent_selection` is correct. `"inherit"` is
passed through verbatim to `AgentQueryConfig.model`, and the engine /
inference layer handles it (teammate path resolves via
`resolve_teammate_model`, standalone path falls through to
`current_main_model_id()` when `selection.model == None`). The one
inconsistency: when `definition.model = Some("inherit")`, the literal
string `"inherit"` reaches the engine; either decode at the spawn
boundary or guarantee the engine treats `"inherit"` as a magic keyword.
Today both paths work because the engine's model factory normalizes
unknown strings to the main model, but that's defensive luck, not
contract.

### Observation 3 — Things I expected to find missing but didn't

These were on my initial gap list and verification ruled them out:
- **Memory tool auto-inject** — exists at
  `definition_store.rs:300-304,440-455`.
- **`required_mcp_servers` gating** — exists at
  `prompt.rs:195-217` (filtered from the prompt listing) plus
  `spawn.rs::initialize_per_agent_mcp`.
- **SubagentStop firing** — fires at `spawn.rs:910-912` (sync) and
  `spawn.rs:1315-1324` (bg).
- **Frontmatter `hooks:` registration** — registered at
  `spawn.rs:863-870`, cleared at `spawn.rs:917-919`.
- **Fork recursion guard** — `is_in_fork_child` in `fork.rs` is wired
  through the `<fork-boilerplate>` tag check at `agent_tool.rs:~444`.
- **Worktree-fallback agent discovery** — covered via the
  `AgentSearchPaths.project_dirs` injection from `app/cli`.
- **Color persistence** — both per-teammate and per-agent-type caches
  exist (`coordinator/CLAUDE.md` "Color caches").

### Observation 4 — Legacy paths are clean

`grep -r "agent_spawn" coco-rs/ --include="*.rs"` returns one hit
(`app/cli/src/paths.rs` comment); no active code. Same for
`agent_advanced`. The migration completed.

---

## Priority Recommendations (if asked to fix something today)

Order is **impact × confidence**:

1. **P0-1 (fork model cache busting)** — highest impact, smallest fix
   (one `request_model = None` branch in `agent_tool.rs::execute`).
   Add a regression test.
2. **P0-2 (sync cancel propagation)** — high impact when interrupted,
   moderate fix (add `cancel` field, wire `tokio::select!`).
3. **P1-3, P1-4 (mcpServers inline, 2-level walk)** — pair them; both
   touch `frontmatter.rs` + `definition_store.rs`. Each has a clear TS
   anchor and is mechanical.
4. **P0-3 (wildcard memory inject)** — small fix once `tools:
   Option<Vec<String>>` lands; the type change is the bigger move.
5. **P2-1 (verbatim builtin prompts)** — pure mechanical port via
   `include_str!`. Knock out as a batch.
6. **P1-5 (subagent_type schema enum)** — design call (rebuild schema
   per-turn vs spawn-time error). Default to the spawn-time error if
   schema rebuild costs too much.
7. **P1-6 (plan-mode subagent client swap)** — narrow fix in
   `spawn.rs::query_config` construction.
8. Everything in P2 is "should fix, not must fix" — defer until other
   work surfaces a need.

Items I'd explicitly NOT do until asked:
- Re-introducing the `Workflow` tool (P1-2) just to mirror the deny-list
  — wait until `Workflow` itself ships.
- Adding ant-build path for `Agent` (P1-1) — coco-rs's design doc says
  ant cloud routes are non-goals.
- Handoff classifier orchestration (P2-3) — needs product decision, not
  code.

---

## Appendix A — Files Read (Direct Verification)

For audit reproducibility, the following files were opened end-to-end
during this pass (not just grepped):

- `core/tools/src/tools/agent/agent_tool.rs` (lines 1-712)
- `core/subagent/src/filter.rs` (full)
- `core/subagent/src/definition_store.rs` (lines 1-475)
- `core/subagent/src/frontmatter.rs` (full)
- `core/subagent/src/prompt.rs` (lines 1-300)
- `core/subagent/src/fork.rs` (full)
- `core/subagent/src/builtins.rs` (full)
- `core/subagent/src/spawn_resolution.rs` (full)
- `core/subagent/src/coordinator_mode.rs` (full)
- `core/tool-runtime/src/agent_handle.rs` (lines 1-300)
- `core/tool-runtime/src/agent_query.rs` (lines 1-130)
- `coordinator/src/agent_handle/spawn.rs` (lines 1-1407)
- `memory/src/agent_memory.rs` (full)
- `app/cli/src/session_runtime.rs` (around 1163-1290 — bootstrap)
- `app/query/src/engine.rs` (around 1056-1087 — plan-mode swap)
- TS reference:
  - `tools/AgentTool/AgentTool.tsx`
  - `tools/AgentTool/runAgent.ts`
  - `tools/AgentTool/loadAgentsDir.ts`
  - `tools/AgentTool/agentMemory.ts`
  - `tools/AgentTool/forkSubagent.ts`
  - `tools/AgentTool/agentToolUtils.ts`
  - `tools/AgentTool/constants/tools.ts`
  - `utils/model/agent.ts`

## Appendix B — Claims Retracted

Two claims surfaced during sub-agent investigation that direct reads
disproved. Recording so future audits don't repeat them:

1. **"Memory does not auto-inject Read/Write/Edit."** False.
   `definition_store.rs:440-455` (`fn inject_memory_tools`) does exactly
   this, called at line 300-304 when `auto_memory_enabled`. The
   investigation agent only searched the spawn path; the injection
   happens at *load* time so by the time spawn runs, the tools are
   already in `def.allowed_tools`.

2. **"Consumer wiring is incomplete — `agent_spawn.rs` is still active."**
   False. The legacy file is gone (one comment reference in
   `app/cli/src/paths.rs`). The wiring runs through
   `session_runtime.rs:1186-1211` → `engine_builder.rs:702` →
   `engine_prompt.rs:316-369` → `agent_tool.rs:59-78`. The Phase-1
   "deferred wiring" line in `core/subagent/CLAUDE.md` is stale.

Both retractions strengthen the verdict: the gap surface is smaller
than I expected going in. The remaining work is small, sharp fixes — not
a refactor.
