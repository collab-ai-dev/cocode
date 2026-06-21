# Adversarial Review: subagent-parity-audit.md

Date: 2026-05-16
Reviewer mode: hostile. Goal: prove the prior audit
(`docs/coco-rs/subagent-parity-audit.md`) is wrong, incomplete, or
unprincipled. Where I can prove it, I do. Where I can't, I say so.

Constraints from the user: mirror TS, best Rust practice, clear arch,
disregard back-compat & dev cost, optimal solution wins.

---

## Verdict (read this first)

The prior audit's *catalogue* of point fixes is mostly sound but
**under-counts the gap surface by roughly 2x** and **misframes the
severity** of the gaps it does list. The audit treats parity as a list
of constant comparisons; the real divergence is **architectural**:
coco-rs builds a subagent by stringing concatenations and bool gates in
a 900-line `spawn_subagent`, where TS has a multi-step assembler with
explicit priority. As long as that asymmetry stands, every gap is a
candidate to silently regress.

Specific verdicts:

- **3 P0s claimed → actually 5 P0s.** Two were missed (`omit_claude_md`
  dead field, `parent_runtime_snapshot` never populated).
- **6 P1s claimed → at least 10 P1s.** Missed: system-prompt priority
  chain, `enhanceSystemPromptWithEnvDetails` equivalent, `output_file`
  divergence's user-visible impact, sync-spawn event emission, `Resume`
  spawn path verification, `forkSubagent` synthetic definition parity.
- **Architectural critique missing.** The audit doesn't say that the
  spawn pipeline itself is the bug surface. It treats `spawn.rs` as a
  black box and audits its inputs/outputs.
- **Several proposed fixes are not "best Rust practice".** The audit
  proposes adding bools and Option fields where sum types and
  non-optional capture would be correct.
- **Two claims are weaker than stated.** The audit retracted "memory
  not auto-injected" — but the retraction itself missed that the
  injection is at the wrong layer (load-time, not spawn-time).

Everything below substantiates these.

---

## Part 1 — Gaps the audit missed

### Missed-1 (P0). `omit_claude_md` is a dead field

The audit lists `omit_claude_md` only in passing ("frontmatter parser
recognized") and never checks it has a consumer. It does not.

- Defined: `common/types/src/agent.rs:570`
- Parsed: `core/subagent/src/frontmatter.rs:178-179`
- Declared `true` on Explore and Plan builtins:
  `core/subagent/src/builtins.rs:200, 214`
- **Consumed nowhere.** `rg "omit_claude_md" coco-rs/ --type rust`
  outside test/define/parse files returns zero.

TS uses this field at the context-assembly boundary to skip injecting
the project's CLAUDE.md into the subagent's system prompt
(`utils/systemPrompt.ts` and the context-builder). Skipping CLAUDE.md
is a token-budget optimization: Explore and Plan agents read CLAUDE.md
via tools when they need it, so attaching it at every spawn is waste.

**User-visible impact.** Explore and Plan agents have ~2-15 KB of
unnecessary tokens prepended on every spawn, depending on project
CLAUDE.md size. This compounds in `coordinator_mode` where workers fan
out and each pays the cost. Silent. No log line.

**Why this is P0 (not P1):** the audit's framing said "P0 = silent
incorrect behavior under load." Token waste on hot-path agents is
exactly that. The field looking implemented but having no consumer is
worse than the field not existing — readers assume it works.

**Fix shape.** Add `omit_claude_md` to the context-assembly seam that
attaches CLAUDE.md to the subagent's first message. Today that
attachment happens via parent context propagation, so the fix is at
whatever layer renders `<system-reminder>` for nested-CLAUDE.md
discovery. (Audit must trace this; I didn't get there.)

---

### Missed-2 (P0). `parent_runtime_snapshot` is hardcoded `None` at the AgentTool boundary

The audit's "Observation 2 — Multi-LLM story is genuinely transparent"
is wrong by omission. I read the field but did not check if it's
populated.

- Defined: `core/tool-runtime/src/agent_handle.rs:251-252`
- Comment says: "Populated by `AgentTool::execute` from the parent's
  `ApiClient` fingerprint (via
  `coco_inference::ProviderClientFingerprint`) at the production call
  site once the runtime threads it through `ToolUseContext`."
- Actual production call site: `core/tools/src/tools/agent/agent_tool.rs:598-600`:
  ```rust
  // the parent's `ProviderClientFingerprint` (T5 follow-up),
  // …
  parent_runtime_snapshot: None,
  ```

The audit retracted "consumer wiring is incomplete" but should have
noticed this one specific consumer is *deliberately* not wired. The
T5-follow-up comment in production code is a self-acknowledged
unfinished bridge.

**User-visible impact.** When `RuntimeConfig` hot-reloads (the user
edits `~/.coco/model.json` mid-session), the parent's `Main` model
changes. A subsequent fork spawn calls `current_main_model_id()`
(`spawn.rs:773`) which reads the *new* main, not the *old* main the
parent has been chatting with. The fork's prompt-cache key now belongs
to a different model — cache miss, full re-tokenize. Worse, if the
provider also changed (Anthropic → OpenAI), the fork runs on a
different vendor entirely, which is the opposite of what fork mode
promises.

**Why this is P0 (not just architectural debt):** the
`parent_runtime_snapshot` field is *only* useful for fork mode — and
fork mode's whole reason to exist is cache parity. The field's absence
defeats the feature's purpose. The fact that this is "tracked as a
follow-up" in code is not an excuse; production code with a known-broken
cache-parity path is broken.

**Fix shape.** Wire `ctx.parent_runtime_snapshot: Option<SubagentRuntimeSnapshot>`
into `ToolUseContext` (it's `Option` because most tools don't need it),
plumb it from the active `ApiClient` at engine bootstrap, and assert
non-None in the `SpawnMode::Fork` branch (a fork without parent runtime
identity is a bug, not a degraded mode).

---

### Missed-3 (P1). No system-prompt priority chain

The TS `buildEffectiveSystemPrompt` has a 6-step priority chain
(`utils/systemPrompt.ts:41-123`):
1. Explicit `override` (set by SDK or test harness)
2. Coordinator system prompt (when leader is coordinator)
3. Agent definition's `system_prompt` (replaces default unless
   "proactive mode")
4. Custom system prompt (`--system-prompt` CLI flag)
5. Default system prompt
6. Append system prompt (`--append-system-prompt`)

The Rust path at `coordinator/src/agent_handle/spawn.rs:680-762`
does:
```rust
let (mut system_prompt, fork_context_messages, preserve_tool_use_results) =
    match request.spawn_mode { ... };
// then …
if inject_memory && def.memory_scope.is_some() {
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&memory_block);
}
```

That's *two* sources concatenated. Where does coordinator-mode prompt
go? Where does `--system-prompt` go? Where does the env-details block
go (TS's `enhanceSystemPromptWithEnvDetails`)? Where does the agent
identity / critical-system-reminder go?

`grep -n` shows:
- `core/subagent/src/coordinator_mode.rs::coordinator_system_prompt()` is
  defined but not called from `spawn.rs`. It's wired from
  `app/query/src/engine_prompt.rs` for the leader, not for subagents.
- No `enhance_system_prompt_with_env_details` equivalent in
  `coordinator/spawn.rs` or anywhere in the spawn path.

**User-visible impact.** Subagent system prompts are missing:
- env details (OS, shell, git status)
- the global `--system-prompt` / `--append-system-prompt` flags
- coordinator-mode override (for nested coordinator scenarios — rare
  but valid)

For typical Explore / Plan subagent use, this means the agent is less
context-aware than its TS twin. For `--system-prompt` users this is a
silent feature loss.

**Why P1, not P0:** the gap is real but most users don't notice because
they don't use `--system-prompt` and Explore/Plan get env info from
their own tool calls. Still, parity says assemble the prompt the same
way TS does, with the same priority order, or document the divergence.

**Fix shape (and "best Rust practice" critique):** today the assembly
is inline ad-hoc string concatenation in `spawn.rs`. Best practice is a
`SystemPromptAssembler` struct in `coco-context` (which already exists
for the leader) with an explicit chain:

```rust
pub struct SystemPromptAssembler<'a> {
    override_: Option<&'a str>,
    coordinator_mode: bool,
    definition: Option<&'a AgentDefinition>,
    custom: Option<&'a str>,
    default_: &'a str,
    append: Option<&'a str>,
    env_details: &'a EnvDetails,
    memory_block: Option<&'a str>,
    skills_preamble: Option<&'a str>,
}

impl<'a> SystemPromptAssembler<'a> {
    pub fn build(&self) -> String { /* priority chain */ }
}
```

Subagent spawn and leader bootstrap call `.build()` on the same
assembler with different inputs. One source of truth. Today there
are at least 4 places that contribute to the final prompt and the
order is whatever `spawn.rs` happens to do.

---

### Missed-4 (P1). `enhanceSystemPromptWithEnvDetails` has no Rust subagent counterpart

Related to Missed-3 but worth listing separately because the symptom is
distinct: the subagent does not get OS, shell, git status, etc. injected
into its system prompt. TS does (`utils/systemPrompt.ts:88-123`).

Verified absence: `rg "enhance_system_prompt|env_details_block|inject_env"
coco-rs/ --type rust` returns hits only in test fixtures, not in
spawn-path code.

Impact: agents that need to make platform-aware decisions
(e.g. choosing `ls` vs `dir`, choosing shell quoting, branching on git
state) have less information. They can fetch it via tools, but the
parent already has it; subagents should inherit.

---

### Missed-5 (P1). `forkSubagent.ts` defines a synthetic agent definition that Rust replaces with inline rules

TS has `tools/AgentTool/forkSubagent.ts:60-71` declaring a synthetic
`AgentDefinition` for the fork agent:
```typescript
{
  agentType: "fork-subagent",
  whenToUse: "…",
  tools: ['*'],
  model: 'inherit',
  permissionMode: 'bubble',
  systemPrompt: forkSystemPrompt(),
}
```

Rust handles fork as a `SpawnMode::Fork` variant and inlines the rules
into `core/subagent/src/fork.rs` (the 10 boilerplate rules) without a
synthetic definition. This is arguably *better* design — fewer phantom
agent types — but it diverges from TS shape: the catalog `/agents list`
in TS would mention "fork-subagent" (in `ant` builds) while Rust
doesn't.

Lower-impact divergence; flagging because the audit's "Observation 3 —
Things I expected to find missing but didn't" implicitly endorsed this
without acknowledging the shape mismatch.

---

### Missed-6 (P1). Sync spawn emits no streaming events to the parent

TS sync subagents emit per-tool-use events to the parent's stream so the
UI shows nested tool calls live. Rust sync path:

- `spawn.rs:903` calls `engine.execute_query(&effective_prompt, query_config).await`
- `AgentQueryConfig` has no `event_tx` field (background path sets it
  at `spawn.rs:838` but sync doesn't pass one in `query_config`).
- Result: parent UI shows "Agent tool: in progress" with no nested
  detail until the spawn completes.

For UX parity this is significant — users routinely watch subagent
tool calls to know what's happening. Easy to miss because the spawn
"works."

**Fix shape.** `AgentQueryConfig` should always carry an
`Option<mpsc::Sender<CoreEvent>>` and the sync path should pipe events
into the parent's stream. (Per the project's "3-layer event dispatch"
convention, the right layer is `Stream` for nested tool calls.)

---

### Missed-7 (P1). `SpawnMode::Resume` is in the enum but is the path actually tested end-to-end?

`SpawnMode::Resume { parent_messages }` is defined
(`agent_handle.rs:102-117`) and used in `spawn.rs:702-704`. But:
- No integration test (that I found) exercises the full path:
  background spawn → JSONL transcript → process restart → Resume from
  JSONL.
- The transcript filter (`core/subagent/src/transcript.rs`) and
  resume-path code (`coordinator/src/agent_handle/resume.rs`) are unit
  tested in isolation but not against a real JSONL produced by a real
  background spawn.

TS has a single integration entry point (`tools/AgentTool/resumeAgent.ts`)
covering both fork-resume and non-fork-resume. Rust has the parts but I
can't confirm they compose.

**Recommendation.** Either add a single end-to-end test that does
spawn → kill → resume, or document in `coordinator/CLAUDE.md` that the
Resume path is not yet exercised end-to-end. Today it claims it is.

---

### Missed-8 (P2). `output_file` and `transcript` are split in Rust; TS symlinks them

The prior audit noted this as a "deliberate divergence per
coordinator/CLAUDE.md." That's true. But the audit didn't flag the
user-visible consequence: tools that follow the TS convention of
**reading the agent's output by tailing the JSONL** (e.g.
`TaskOutput`) won't see streamed text deltas in Rust because the deltas
go to `.output` (a separate file), not the JSONL. Anyone porting a TS
workflow that tails JSONL for live agent output gets nothing.

**Recommendation.** Either restore symlink parity (TS-compatible) or
document explicitly that `TaskOutput` in Rust reads `.output` and
provide a migration note. The current `coordinator/CLAUDE.md` line ("…
cleaner UX") is one-sided; the migration cost wasn't acknowledged.

---

### Missed-9 (P2). Tool result envelope: how is the final result extracted?

TS's subagent result is the last assistant text block, with specific
handling for tool_use blocks (omitted) and thinking blocks (stripped).
Rust populates `AgentSpawnResponse.result` from `qr.messages` after
`engine.execute_query`. Where is the extraction logic? Without seeing
it I can't audit whether thinking blocks leak, tool_use blocks are
correctly stripped, or empty results trigger `EMPTY_AGENT_OUTPUT_MARKER`
correctly. Spawn returns `qr.messages` essentially — but the conversion
from messages → text is delegated implicitly. The audit didn't audit
this.

**Action.** Trace the extraction from `QueryResult.messages` to
`AgentSpawnResponse.result`. The string the parent model sees is the
key contract.

---

### Missed-10 (P2). `PerCallOverrides` (temperature, thinking budget, etc.) inheritance unclear

`AgentSpawnRequest` carries `effort: Option<String>` but not
`temperature`, `top_p`, `thinking_budget`. TS forwards an
`AgentToolOptions` shape that includes those. If a parent sets
`temperature: 0.2` in its options, does a subagent inherit that? I
don't know — that's the gap.

**Action.** Decide and document: subagents inherit parent's per-call
overrides unless their definition declares its own (the safe default,
matching TS), or subagents reset to runtime defaults (cleaner).
Whatever the choice, encode it in `AgentSpawnRequest` explicitly
rather than leaving fields out.

---

## Part 2 — Where the audit's existing entries are wrong or weak

### Audit P0-1 (Fork model not pinned) — Right symptom, wrong fix

I claimed: "force `request_model = None` in fork branch."

That's not best Rust practice. Forcing a field's value via a runtime
branch is exactly the pattern type systems exist to prevent. Better:

- Make `SubagentSelection::for_fork(parent_snapshot: ParentRuntimeSnapshot)`
  a constructor that doesn't accept any caller-supplied model
  override.
- `SubagentSelection::for_fresh(request_model, definition, subagent_type)`
  is the other constructor.
- `resolve_subagent_selection` becomes either a `_fresh` dispatcher
  or is deleted entirely.

This also forces the caller to *have* a `ParentRuntimeSnapshot` for
fork, which closes the Missed-2 gap structurally. Two bugs, one type.

### Audit P0-2 (Sync cancel) — Right symptom, incomplete fix

I claimed: add `cancel: CancellationToken` to `AgentSpawnRequest`,
wire `tokio::select!`.

That alone doesn't propagate the cancel **into the child engine's tool
execution**. The child engine's `ToolUseContext.cancel` field is a
separate `CancellationToken`; if it's constructed fresh inside
`AgentQueryEngine::execute_query`, then dropping the outer future just
cancels the outer await — the child's tools may continue running for a
turn.

Right fix has three steps:
1. Carry the parent's token (or a child of it) on `AgentSpawnRequest`.
2. Have `AgentQueryEngine::execute_query` accept it.
3. Have the engine pass the *same* token (or a child) into every
   `ToolUseContext` it constructs.

Plus the `tokio::select!` outer race. Without all three, cancel is
half-cancel.

### Audit P0-3 (Wildcard memory inject) — Right diagnosis, wrong type design

I proposed adding `wildcard_tools: bool` alongside the existing
`allowed_tools: Vec<String>`.

That's the worst of both worlds. The two fields can desync. Best
practice: replace `allowed_tools: Vec<String>` with:

```rust
pub enum ToolAllowList {
    /// `tools: ['*']` or `tools: []` (TS treats both as wildcard;
    /// but Rust should distinguish via the parse layer if we ever
    /// want them to differ).
    Wildcard,
    /// Explicit allow-list.
    Explicit(Vec<ToolName>),  // note: ToolName, not String
}
```

`inject_memory_tools` becomes:
```rust
fn inject_memory_tools(def: &mut AgentDefinition) {
    let ToolAllowList::Explicit(list) = &mut def.allowed_tools else {
        return;  // wildcard: nothing to do
    };
    if def.memory_scope.is_none() { return; }
    for tool in [ToolName::Read, ToolName::Edit, ToolName::Write] {
        if !list.contains(&tool) { list.push(tool); }
    }
}
```

Plus a separate `Stringly` → `ToolName` parse in `frontmatter.rs` that
errors on unknown names (today they're silently kept as strings).

### Audit's retraction of "memory not auto-injected" — Retraction is correct, but the implementation is at the wrong layer

The audit retracted the gap because `inject_memory_tools` runs at
`definition_store::load()` (line 300-304). That's true. But:

- `auto_memory_enabled` is captured at load time, never re-read.
- If the user toggles auto-memory via `/settings` mid-session, the
  cached definitions don't update.
- The store has no notification of the toggle change.

So the *behavior* is right under stable config, but the design is
wrong: the injection should happen at spawn time, with the live
`Features` state, not at load time with a captured bool. Today this
is hidden because nothing in the codebase toggles `auto_memory_enabled`
at runtime — but the moment that becomes possible, the bug surfaces.

This is "best Rust practice and clear arch" territory: load-time
mutation of definitions based on runtime config is a smell.
Definitions should be immutable post-parse; capability decisions
happen at the consumption point.

### Audit P1-1 (`Agent` in deny-list, ant divergence) — Should be deleted, not listed

For coco-rs's stated 3P scope (no ant cloud routes per CLAUDE.md), the
divergence has no users. Listing it as a P1 cluttered the list. The
honest framing: "this divergence is intentional given the project's
non-goals; no fix planned."

### Audit P1-4 (1-level dir walk) — Solution is fine, classification is wrong

Promoting this to P0 makes sense: agent files in `agents/refactor/`
disappearing from the catalog is silent and visible. Users who use the
agents/ subfolder convention see fewer agents than they wrote.
Recategorize.

### Audit's "Priority Recommendations" — Misses dependency graph

The list orders by impact × confidence but doesn't say:
- P0-3 (wildcard memory) depends on the `AgentDefinition.allowed_tools`
  type change.
- That type change blocks P1-5 (subagent_type schema enum) because
  schema generation needs the new type.
- P0-1 and Missed-2 (`parent_runtime_snapshot`) want to be paired —
  fix the runtime snapshot first, then the fork model fix becomes
  one-liner.
- Missed-3 (system prompt assembler) is a refactor that unblocks
  P1-6, Missed-3, Missed-4 simultaneously.

A correct priority graph is:
```
1. ParentRuntimeSnapshot wiring (Missed-2)
   └─ enables P0-1 (fork model) as a one-liner
2. SystemPromptAssembler refactor (Missed-3)
   ├─ enables Missed-4 (env details)
   ├─ enables P1-6 (plan-mode subagent)
   └─ enables Missed-1 (omit_claude_md) consumer
3. ToolAllowList sum type (P0-3 reframed)
   ├─ enables P0-3 wildcard fix
   └─ enables P1-5 (schema enum)
4. Cancel token propagation (P0-2)
5. Inline mcpServers parser (P1-3) + walkdir (P1-4)
6. Event piping (Missed-6) + Resume e2e test (Missed-7)
```

---

## Part 3 — Architectural critiques the audit failed to make

### Arch-A. `spawn_subagent` is a god-function

`coordinator/src/agent_handle/spawn.rs::spawn_subagent` is the dispatch
point for 12+ responsibilities: validation, worktree, identity,
agent-state commit, definition resolution, model resolution, spawn-mode
dispatch, system-prompt assembly, memory injection, hook
registration, MCP setup, skill preload, hook firing, query execution,
post-spawn classification, summary, cleanup. The function is over 700
lines.

Best Rust practice: split into a `SpawnPipeline` of `SpawnStep`s, each
a trait method receiving a mutable `SpawnContext`:

```rust
trait SpawnStep: Send + Sync {
    async fn run(&self, ctx: &mut SpawnContext) -> Result<(), SpawnError>;
}

struct SpawnPipeline {
    steps: Vec<Box<dyn SpawnStep>>,
}
```

Steps: `ValidateRequest`, `AssembleSystemPrompt`, `ResolveDefinition`,
`AcquireWorktree`, `RegisterFrontmatterHooks`, `InitMcpServers`,
`PreloadSkills`, `FireSubagentStart`, `RunQuery`, `FireSubagentStop`,
`CleanupHooks`, `CleanupMcp`, `CleanupWorktree`, `ClassifyHandoff`,
`Summarize`.

The current shape makes it impossible to unit-test step interactions
or to inject test doubles for individual steps.

### Arch-B. `AgentSpawnRequest` is a 27-field bag with mixed concerns

The request mixes:
- Caller-supplied inputs (prompt, description, subagent_type, model)
- Inheritance hints (`features`, `tool_overrides`, `parent_tool_filter`,
  `parent_runtime_snapshot`)
- Layer-2 derived data (`fork_context_messages`, `spawn_mode`,
  `definition`)
- Per-fork constraints (`constraints`, `can_use_tool`,
  `require_can_use_tool`)
- Telemetry labels (`fork_label`)
- Persistence policy (`skip_transcript`)

Best Rust practice: nested structs by concern.

```rust
pub struct AgentSpawnRequest {
    pub call: AgentCallInput,           // model-supplied JSON
    pub inheritance: AgentInheritance,  // from parent runtime
    pub spawn_mode: SpawnMode,
    pub safety: AgentSafetyConfig,      // constraints + can_use_tool
    pub telemetry: AgentTelemetryConfig,
    pub persistence: AgentPersistenceConfig,
}
```

This makes it impossible to forget to pass inheritance (the struct
exists or it doesn't; the field can't be hand-defaulted to `None`).

### Arch-C. `#[serde(skip)]` on inheritance fields is a footgun

Half of `AgentSpawnRequest`'s fields are `#[serde(skip)]`. The struct
serializes to JSON missing those fields, deserializes back without
them. If the request ever crosses an IPC boundary (e.g., a remote
worker via NDJSON), the inheritance is silently lost and the
deserialized request runs with default everything.

Best practice options:
- Make the struct non-`Serialize` (compile-time prevents the bug).
- Split into a `Wire` form (what crosses IPC) and a `Runtime` form (in-process).

Today both `serde(skip)` and `Deserialize` are present, suggesting
the type might want to cross IPC but the inheritance fields are
quietly dropped — exactly the bug pattern best practice prevents.

### Arch-D. `SpawnMode` is `#[non_exhaustive]` but has only internal callers

`#[non_exhaustive]` is for public API stability across crate-version
boundaries. Internally, the compiler should force every variant to be
handled. The current `other =>` wildcard in `spawn.rs:710-720` treats
unknown variants as `Fresh`, which is a footgun — adding a variant
upstream silently passes through as `Fresh` here.

Best practice: remove `#[non_exhaustive]` from internal enums, force
compile errors. Add it back only if/when the type genuinely needs to be
cross-version-stable across crate boundaries (`SpawnMode` doesn't —
it's used by one consumer in one crate).

### Arch-E. Tool names are `Vec<String>` everywhere; should be `Vec<ToolName>`

`AgentDefinition.allowed_tools: Vec<String>` — a stringly-typed list of
tools where every comparison is a `&str` equality. Add a typo to a
custom agent's frontmatter and the tool just silently doesn't load.

Best practice: `Vec<ToolName>` (the enum exists in `coco-types`).
Frontmatter parser errors on unknown names instead of silently
keeping them. MCP tools (the `mcp__*` prefix family) become a
variant: `ToolName::Mcp { server: String, tool: String }` or similar.

This change is invasive (touches the filter, the prompt renderer, the
schema, the registry), but it's the right one if "best Rust practice"
is the bar.

### Arch-F. Two parallel "is this a fork" decisions

- `coco_subagent::is_fork_subagent_active(features, is_non_interactive)`
- `matches!(spawn_mode, SpawnMode::Fork { .. })`

Caller has to keep them consistent. They can drift. Right now
`agent_tool.rs:422-424` checks both:
```rust
let spawn_mode = if subagent_type.is_none()
    && coco_subagent::is_fork_subagent_active(&ctx.features, ctx.is_non_interactive)
{ … SpawnMode::Fork { … } } else { SpawnMode::Fresh };
```

If anyone elsewhere constructs `SpawnMode::Fork` without going through
this gate, the gate is bypassed. Best practice: make `SpawnMode::Fork`
*only* constructible via `coco_subagent::try_fork(features,
is_non_interactive, …) -> Option<SpawnMode>`. The enum's variant is
`pub(crate)` to the subagent crate so external construction is
impossible.

### Arch-G. `ToolFilterPlan` is computed but applied elsewhere

`AgentToolFilter::plan()` returns a `ToolFilterPlan` describing what's
allowed. The actual application — narrowing the child's `ToolRegistry`
— happens in `app/query/src/agent_adapter.rs:201-209` per the docs.
This split means a caller can produce a plan and ignore it. Best
practice: the plan and the registry-narrowing should be inseparable. A
single `ToolRegistry::narrowed_by(plan: &ToolFilterPlan) ->
ToolRegistry` method, and the plan's `allowed_tools` field should
not be `pub` — the only legitimate consumer is `narrowed_by`.

### Arch-H. Definition load-time injection is a layering smell

`inject_memory_tools` runs in `definition_store::load`. The
definition store is supposed to be a pure parser; the moment it
auto-modifies parsed data based on runtime config (`auto_memory_enabled`)
it's not pure anymore. Future readers will be confused about what's in
the .md file vs what's been added.

Best practice: keep `AgentDefinition` representative of the source-of-truth
.md. The spawn step that needs Read/Edit/Write computes the
*effective* allow-list at use time. The pure-logic store stays pure.

---

## Part 4 — Things the audit overstated

### Overstated-1. "29/31 filter constants identical" — true but trivial

The audit boasted 16/16 ASYNC_AGENT_ALLOWED_TOOLS parity and 8/8
IN_PROCESS_TEAMMATE parity. Those are constant lists; identity is
table-stakes, not a parity achievement. Better framing: "filter
*constants* match; filter *semantics* deviate at the gating-pattern
level (the conditional inclusion of Agent and Workflow has no Rust
equivalent)."

### Overstated-2. "Consumer wiring is real" — true but incomplete

I retracted "consumer wiring is incomplete" because the catalog is
loaded and threaded. But `parent_runtime_snapshot` is one specific
consumer that *is* unwired, and `omit_claude_md` is another (Missed-1).
The retraction was binary when the truth is "most consumers wired, two
material ones not."

### Overstated-3. "Multi-LLM story is genuinely transparent"

Without `parent_runtime_snapshot` (Missed-2), it's transparent only as
long as `RuntimeConfig` doesn't hot-reload. The audit framed
hot-reload as a feature ("the spawn picks up the new Main mapping —
T6") without acknowledging that mid-conversation provider switches
break fork mode invariants.

### Overstated-4. "Built-in roster matches TS"

Section "P2-1" of the audit acknowledged paraphrased `whenToUse`
strings. But beyond `whenToUse`, the entire `system_prompt` body for
each builtin is paraphrased (`builtin_prompts.rs` is hand-written
Rust prose, not `include_str!` of TS source). For evaluations,
distillation, or behavioral diffing against TS, this is a contract
violation. The audit hid this under "P2 quality-of-implementation,"
which buries the lede.

---

## Part 5 — Recommended scope realignment

If the goal is "mirror TS, best Rust practice, clear arch, optimal
solution wins," the audit's flat list of fixes is the wrong shape. The
right shape is two parallel tracks:

### Track 1: Architecture (do these in order)

1. **`SystemPromptAssembler`** — extract from `spawn.rs` + leader path.
   Closes Missed-3, Missed-4, P1-6, Missed-1 (omit_claude_md consumer).
2. **`AgentSpawnRequest` decomposition** — nested concern-grouped
   structs. Forces `parent_runtime_snapshot` to be non-optional in the
   fork constructor. Closes Missed-2 + P0-1 structurally.
3. **`ToolAllowList` sum type + `ToolName` for `allowed_tools`** —
   closes P0-3, P1-5, fixes silent unknown-tool acceptance.
4. **`SpawnPipeline` of `SpawnStep`s** — closes Arch-A. Makes every
   subsequent fix testable in isolation.
5. **`ToolRegistry::narrowed_by(plan)` enforcement** — closes Arch-G.
6. **`SpawnMode` non-`#[non_exhaustive]` + private construction** —
   closes Arch-D, Arch-F.

### Track 2: Plumbing (do these after Track 1 lands)

1. Cancel token propagation (P0-2) with all three steps.
2. `mcpServers` inline form parser (P1-3).
3. `walkdir`-based 2-level recursive directory walk (P1-4, reclassified
   to P0).
4. Sync-spawn event piping (Missed-6).
5. Resume path e2e test (Missed-7).
6. `output_file`/transcript symlink restoration OR explicit migration
   doc (Missed-8).
7. Tool result extraction audit (Missed-9).
8. `PerCallOverrides` inheritance decision (Missed-10).
9. Verbatim builtin prompt strings via `include_str!` (P2-1 promoted).
10. Stale doc cleanup — delete the "deferred Phase-1 wiring" lines in
    `core/subagent/CLAUDE.md` that are no longer true.

### Track 3 (defer or close):

- P1-1 (`Agent` ant divergence): close as "intentional given non-goals."
- P1-2 (`Workflow` tool): defer until `Workflow` ships.
- P2-3 (handoff classifier orchestration): close pending product
  decision.

---

## Part 6 — What I still don't know

Honest list of gaps in this review itself:

1. I did not trace `AgentSpawnResponse.result` extraction from
   `QueryResult.messages`. The audit didn't either. This is the
   single string the parent model sees; an audit that doesn't check it
   is incomplete.

2. I did not check `cwd_override` propagation into all downstream
   contexts (env block, system reminder, file-path resolution in tool
   call). The single `cwd_override` field is set, but where it's read
   is not enumerated.

3. I did not run any of the tests. The retracted "agent_spawn.rs is
   still active" claim was wrong because I trusted the prior agent's
   grep; I confirmed via my own grep but I haven't *run* anything to
   confirm the store's snapshot is actually consumed by the prompt
   renderer in a live session.

4. I did not audit `permission_mode` resolution for non-trivial cases.
   The `coco_permissions::resolve_subagent_mode` function is called but
   its behavior across the `plan|review|acceptEdits|bubble` ×
   `parent_mode` matrix is not verified.

5. I did not check whether the `T5`/`T6`/`T7` comments scattered through
   the code (`agent_tool.rs:598`, etc.) represent a coherent migration
   plan with a tracker, or are stale TODOs. They look like the latter.
   That itself is a documentation-debt smell worth surfacing.

6. I did not test the `<task-notification>` XML round-trip under
   adversarial inputs (malformed worker output, partial XML, embedded
   `<task-notification>` in user prose, etc.). The serialization parity
   was verified at the constant level only.

7. I did not verify that frontmatter `hooks` registration is correctly
   *scoped* — the audit confirmed registration fires, but didn't
   verify the hook runs only for that subagent's events (and not for
   sibling subagents or the parent's events).

Closing these gaps is a follow-up audit, not part of this review.

---

## Bottom line

The prior audit found real bugs but underestimated the surface and
proposed fixes that, while correct in spirit, don't meet the "best
Rust practice and clear arch" bar the user set. The right framing isn't
"here are 13 fixes" — it's "here are 6 architecture changes, after
which the 13 fixes become 4 fixes, and 9 of them disappear or become
mechanical."

If the user really means "optimal solution wins, disregard back-compat
and dev cost," then the architecture changes (Track 1) are non-optional.
A pile of point fixes on the current shape preserves the structural
smells that produced the bugs in the first place.
