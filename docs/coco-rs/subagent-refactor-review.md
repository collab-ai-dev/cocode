# Adversarial Review: 2026-05-16 Subagent Refactor

Reviewer stance: hostile. Constraints: mirror TS, best Rust practice,
clear arch, disregard back-compat & dev cost, optimal solution wins.

Target of review:
- PR 1.1 — env block + AGENT_NOTES + per-model knowledge_cutoff
- P0-1 — fork-mode model pinning
- Missed-2 — `parent_runtime_snapshot` wiring
- PR 2.1 — subagent reuses `build_system_prompt`
- P1-6 — plan-mode child swap
- AGENT_NOTES routing fix (mid-session correction)

Verdict up front: the refactor closed several real gaps but introduced
**two new TS-parity regressions** and **at least seven sub-optimal
shape choices**. Specifically PR 2.1 actively diverges from TS in a
way the prior review and my "AGENT_NOTES correction" both missed.

---

## P0 (regressions — must fix)

### P0-A. PR 2.1 injects CLAUDE.md into subagent system prompt — TS does NOT

**Severity rationale.** This is the headline regression. Pre-refactor,
`spawn.rs` built the subagent SP as `def.system_prompt + memory_block`
— no CLAUDE.md. That **matched TS** (TS subagent SP =
`[agentPrompt, notes, envInfo]`, no CLAUDE.md). My PR 2.1 routes
through `coco_context::build_system_prompt` which calls
`discover_memory_files(cwd)` and embeds the CLAUDE.md content as
"Project Instructions" inside the SP. Subagents now carry every
parent CLAUDE.md byte in their system prompt — completely contrary
to TS design.

**TS evidence.**
`tools/AgentTool/AgentTool.tsx:518` returns
`agentPrompt = selectedAgent.getSystemPrompt({...})` (just the agent's
own role text, no CLAUDE.md). Then
`enhanceSystemPromptWithEnvDetails([agentPrompt], ...)` at line 534
appends only `[notes, envInfo]` — three elements total, no CLAUDE.md.

CLAUDE.md flows to TS subagents via **`userContext`** (a different
mechanism — first-turn user message attachment), not via system prompt.
`runAgent.ts:380-398`:
```ts
const baseUserContext = override?.userContext ?? getUserContext()
const shouldOmitClaudeMd = agentDefinition.omitClaudeMd && !override?.userContext && …
const { claudeMd: _omitted, ...userContextNoClaudeMd } = baseUserContext
const resolvedUserContext = shouldOmitClaudeMd ? userContextNoClaudeMd : baseUserContext
```

`omitClaudeMd` strips CLAUDE.md from the **userContext**, not the
system prompt.

**Coco-rs collateral.** My implementation:
1. Adds CLAUDE.md to the subagent SP (wrong place).
2. Consumes `omit_claude_md` by skipping `discover_memory_files` (wrong
   gate — the field's purpose is to gate the *userContext* path, not
   the SP).

**Result.** A subagent like `Explore` declares `omit_claude_md: true`
because TS docs say it saves "~5-15 Gtok/week" by avoiding CLAUDE.md.
In TS this gate works on the userContext attachment. In my coco-rs
build it works on the SP injection. Wrong layer; the saving claim
becomes meaningless (it was measured against a TS code path that
doesn't exist here).

**Fix shape.**
1. Pass `&[]` (empty slice) for `claude_md_files` in the subagent
   `build_fresh_prompt` call. Subagent SP gets identity + env + notes
   only — TS shape.
2. Move `omit_claude_md` consumption to wherever coco-rs delivers
   CLAUDE.md to subagents via attachments (if it does — needs audit).
   If coco-rs already attaches CLAUDE.md via nested memory triggers
   for subagents, `omit_claude_md` gates THAT.
3. If coco-rs does NOT yet attach CLAUDE.md to subagents via
   userContext-equivalent, then port that mechanism — without it the
   "PR 2.1 gives subagents CLAUDE.md" was already filling a real gap
   wrong-shaped. Removing it without adding the right shape is a
   regression in coverage.

This needs to be reverted-and-replaced, not patched.

---

### P0-B. AGENT_NOTES positioned at the end of system prompt — TS positions it BEFORE env block

**Severity rationale.** TS `enhanceSystemPromptWithEnvDetails` returns
`[agentPrompt, notes, envInfo]`. Order matters for model attention —
TS puts behavior rules BEFORE the env block.

My "fix" routes AGENT_NOTES via the `custom_append` slot in
`build_system_prompt`. Looking at the function body, `custom_append`
is added LAST — after env, after skills, after memory. So in my
implementation the model sees:

```
identity
(output_style if set)
CACHE_BREAKPOINT
(CLAUDE.md if files exist — also wrong per P0-A)
env_block
(skills if set)
CACHE_BREAKPOINT
(memory_block if set)
AGENT_NOTES        ← appended here
```

TS sees:

```
agentPrompt
notes              ← right after identity
envInfo
```

**Model-facing impact.** Behavior rules ("absolute paths only", "no
colon before tool calls") are 5+ blocks downstream of identity in my
implementation, vs. immediately adjacent in TS. Quantifiable effect
on model adherence is unknown but the divergence is real.

**Fix shape.** Don't piggyback on `custom_append`. Either:

- **Option 1 (proper):** Add an explicit `notes: Option<&str>`
  parameter to `build_system_prompt` and render it between identity
  and CLAUDE.md.
- **Option 2 (decouple):** Drop `build_system_prompt` reuse for
  subagents entirely; build subagent SP at the spawn site with
  explicit TS-mirroring order: `[identity, AGENT_NOTES, env_block,
  memory_block].join("\n\n")`. Skip `coco_context::SystemPrompt`'s
  cache-breakpoint machinery (TS subagents don't use multi-block
  system prompt either — see P1-A).

Option 2 is more honest. Sharing the assembler was a tempting
DRY-ism that doesn't pay off because the two paths produce different
shapes.

---

## P1 (TS-parity divergences)

### P1-A. `SystemPrompt` cache-breakpoint structure is lost via `.full_text()`

`build_system_prompt` returns `SystemPrompt { blocks: Vec<...> }` with
explicit `CacheBreakpoint` markers. My spawn-path code calls
`.full_text()` and stores the result as a flat `String` on
`AgentQueryConfig.system_prompt`. The breakpoint information is
discarded.

Pre-existing limitation (`AgentQueryConfig.system_prompt` is `String`,
always was). But my refactor exposes the inconsistency: the leader
also calls `.full_text()` at `headless::build_system_prompt`. Both
paths throw away the cache structure.

TS preserves multi-block SP for Anthropic's `cache_control` per-block
markers (`provider_options.anthropic.cacheStrategy`). The lost
structure means coco-rs doesn't get per-block cache control even
though `coco_context::SystemPrompt` was designed for it.

**Fix shape.** Change `AgentQueryConfig.system_prompt` from `String`
to `coco_context::SystemPrompt` and have the inference layer emit
multi-block system content with `cache_control` markers. This is a
larger surgery than this refactor's scope, but it's the right
direction.

---

### P1-B. Resume path re-injects memory block — TS doesn't (and shouldn't)

In `spawn.rs::build_fresh_prompt`, memory injection gate is:

```rust
let inject_memory = !matches!(request.spawn_mode, SpawnMode::Fork{..});
```

So Resume DOES inject `memory_block`. But Resume restores a
previously-running agent — its prior turns already contain whatever
memory it wrote. Re-injecting `MEMORY.md` content at every resume
duplicates it in the agent's view.

TS `resumeAgent.ts` rebuilds the system prompt fresh (per the comment
I wrote earlier) but the memory block isn't re-injected; the agent's
existing transcript shows what it already knew.

**Fix shape.** Add Resume to the no-inject set:
```rust
let inject_memory = matches!(request.spawn_mode, SpawnMode::Fresh{..});
```

---

### P1-C. Plan-mode swap also fires for Resume — should preserve original spawn's role

The P1-6 fix forces `ModelRole::Plan` when `mode == "plan"` and
`!matches!(SpawnMode::Fork{..})`. That means Resume + plan-mode →
forces Plan role.

But Resume restores an agent that was originally spawned with a
specific role (probably saved in JSONL metadata). Overriding that role
on resume breaks "Resume reproduces original conversation" invariant.

**Fix shape.** Same `matches!(Fresh{..})` guard as memory:
```rust
model_role: Some(
    if request.mode.as_deref() == Some("plan")
        && matches!(request.spawn_mode, SpawnMode::Fresh { .. })
    {
        ModelRole::Plan
    } else {
        selection.model_role
    },
),
```

---

### P1-D. `model_for_env` for Resume uses parent's current model — should use the resumed agent's original model

My code:
```rust
let model_for_env = if matches!(SpawnMode::Fork | SpawnMode::Resume) {
    request.parent_runtime_snapshot.api_model_name
};
```

Resume case: the resumed agent was originally spawned with some
model. Its JSONL transcript records what it saw last turn. If the
parent has since hot-reloaded RuntimeConfig, the `parent_runtime_snapshot`
captures the NEW parent identity, not the historical agent identity.

Showing "You are powered by the model X" in the env block where X is
the new parent's model — but the agent is actually still configured
to keep talking to the OLD model (loaded from JSONL config) — is a
self-contradiction.

**Fix shape.** `SpawnMode::Resume` should carry its own
`resumed_snapshot: ParentRuntimeSnapshot` (loaded from
`agent-<id>.metadata.json` per coordinator/CLAUDE.md). Use that for
`model_for_env` in Resume, not the parent's snapshot.

---

### P1-E. `parent_runtime_snapshot` computed per-batch — should be cached at engine bootstrap

`engine_prompt.rs::tool_context_factory()` is called every batch.
Each call does:
```rust
parent_runtime_snapshot: Some(Arc::new(self.client.fingerprint().to_snapshot()))
```

`fingerprint()` is a `&ProviderClientFingerprint` accessor (cheap),
but `to_snapshot()` allocates a new `SubagentRuntimeSnapshot` every
call, then wraps in a new `Arc`. Over a 100-batch session that's 100
allocations of the same content.

**Fix shape.** Cache as a single `Arc<SubagentRuntimeSnapshot>` field
on `QueryEngine` at construction; clone the Arc per batch (free).
If hot-reload of `ApiClient` happens, the engine factory rebuilds
anyway — the snapshot becomes stale only if the engine survives a
client swap, which it doesn't (rebuilt at client swap per CLAUDE.md).

---

## P2 (best-Rust-practice violations)

### P2-A. `custom_append` semantic conflation

`custom_append` was added for `--append-system-prompt` (user-supplied
CLI flag). Piggybacking AGENT_NOTES onto it conflates two concerns:
1. User's append flag (custom; per-session)
2. Subagent behavior rules (constant; per-spawn-class)

The result: if we ever want `--append-system-prompt` to flow to
subagents, the implementation has nowhere to put it without
clobbering AGENT_NOTES.

**Fix shape.** Add a dedicated `notes: Option<&str>` parameter to
`build_system_prompt` (see P0-B fix). Or use Option 2 of P0-B and
keep the function signature clean.

---

### P2-B. `additional_working_directories: &[String]` is stringly-typed

Should be `&[PathBuf]` or `impl IntoIterator<Item = &Path>`. Strings
lose path semantics — relative vs absolute, trailing slash, encoding
— and consumers must re-parse them.

`headless::build_system_prompt_for_model` already converts
`Vec<PathBuf>` → `Vec<String>` at the boundary just to fit my new
signature. Type erosion.

**Fix.** Change parameter type to `&[PathBuf]`. `render_env_block`
uses `.display().to_string()` per entry — no behavior change, but the
API contract is honest.

---

### P2-C. `render_shell_line` reads `Platform::current()` instead of `env.platform`

```rust
fn render_shell_line(shell: ShellKind) -> String {
    if matches!(Platform::current(), Platform::Windows) { ... }
}
```

`Platform::current()` is a runtime probe of the actual host OS.
`env.platform` is the platform recorded in `EnvironmentInfo`. They're
usually identical, but in tests / cross-platform mocks they can
differ. The function should take `env` as input and read
`env.platform`.

**Fix.** Pass `&EnvironmentInfo` instead of `ShellKind` alone, or
take `(shell, platform)` as paired parameters.

---

### P2-D. `Platform::ts_name` is not exhaustive

```rust
pub fn ts_name(&self) -> &'static str {
    match self {
        Self::Darwin => "darwin",
        Self::Linux => "linux",
        Self::Windows => "win32",
    }
}
```

`Platform` enum has exactly 3 variants today, but is the user CLAUDE.md
right that `coco-context` should accommodate future platforms? If
someone adds `Platform::Ios` or `Platform::FreeBSD`, the compiler
will force handling here — which is fine. Less fine: `Platform::Linux`
returns `"linux"` but Node's `os.platform()` may return `"freebsd"`,
`"openbsd"`, `"sunos"` etc. mapped to `Linux` in coco-rs's coarser
enum. This **loses information**.

**Fix.** Either widen `Platform` to mirror Node's full enum
(`darwin`/`linux`/`win32`/`freebsd`/`openbsd`/`sunos`/`aix`/`android`)
or accept that `Platform::Linux → "linux"` is a deliberate coarsening
and document it.

---

### P2-E. `knowledge_cutoff_for_model` substring matching is fragile

```rust
if m.contains("claude-opus-4-7") { Some("January 2026") }
else if m.contains("claude-opus-4-6") || m.contains("claude-opus-4-5") { Some("May 2025") }
```

`"claude-opus-4-7-extended"` matches the first arm. `"claude-opus-4-7-1m"`
also matches. Intentional? Probably yes for variants. But "claude-opus-4"
also matches `"claude-opus-4-7"` via the `contains`. The order matters
and the fallthrough is order-dependent.

TS uses `getCanonicalName(modelId)` BEFORE substring matching to
normalize variant names to a canonical base. My code skips that step.

**Fix.** Either copy TS's `getCanonicalName` first, or use exact-prefix
match with the most-specific variant first (current code already orders
correctly for the listed models, but the policy isn't enforced).

---

### P2-F. `DEFAULT_AGENT_IDENTITY` is likely dead code in practice

All 6 builtin agents set `system_prompt` via `builtin_prompts.rs`. All
markdown agents must have a non-empty body to parse. The `.unwrap_or`
fallback triggers only when:
- `definition` is `None` (catalog miss for an unknown subagent_type)
- Definition exists but `system_prompt` is `None` (parser bug?)

The first case is reachable only after the audit's P1-5 (subagent_type
schema enum) is closed — currently unknown types silently fall back to
`general-purpose` which has a populated `system_prompt`.

**Verdict.** Defendable as a defensive default but worth marking
`#[doc(hidden)]` or `pub(crate)` until something actually reaches it.

---

### P2-G. `build_fresh_prompt` invoked three times for identical output

Match arms in `spawn.rs`:
```rust
SpawnMode::Resume { parent_messages } => (build_fresh_prompt(), parent_messages.clone(), true),
SpawnMode::Fresh => (build_fresh_prompt(), request.fork_context_messages.clone(), false),
other => { tracing::warn!(...); (build_fresh_prompt(), …, false) }
```

All three produce identical strings. The closure body re-reads CLAUDE.md
from disk and re-builds env_info on each call.

**Fix.** Compute once outside the match:
```rust
let fresh_prompt = build_fresh_prompt();
let (system_prompt, fork_context_messages, preserve) = match &request.spawn_mode {
    SpawnMode::Fork { ... } => (fork_bytes, build_fork_context(...).messages, true),
    SpawnMode::Resume { parent_messages } => (fresh_prompt, parent_messages.clone(), true),
    SpawnMode::Fresh => (fresh_prompt, request.fork_context_messages.clone(), false),
    other => { tracing::warn!(...); (fresh_prompt, request.fork_context_messages.clone(), false) }
};
```

Or hoist `build_fresh_prompt` to a method on `SwarmAgentHandle` so it's
not constructed per spawn.

---

### P2-H. Two parallel "model resolution" paths in spawn.rs

Now there are TWO model resolutions:

1. `model_for_env` (for the env block in SP) — lines after PR 1.1 fix:
   ```rust
   let model_for_env = if Fork|Resume { snapshot.api_model_name } else { selection.model or current_main_model_id };
   ```

2. `query_config.model` (for the actual API call) — line 770:
   ```rust
   model: selection.model.unwrap_or_else(|| current_main_model_id())
   ```

They can disagree. For Fork mode:
- `model_for_env` uses `parent_runtime_snapshot.api_model_name`
- `query_config.model` uses `current_main_model_id()` (since P0-1
  stripped the caller `model`, and fork resolves via fallback chain)

The env block says "You are powered by model X" but the actual API
call hits model Y. Silently wrong.

**Fix.** Single resolution function:
```rust
fn resolve_runtime_model(request: &AgentSpawnRequest, selection: &SubagentSelection, handle: &Self) -> String;
```
Both `model_for_env` and `query_config.model` use it.

---

## P3 (lower-priority observations)

### P3-A. Tests for the AGENT_NOTES routing are missing

After the mid-session correction, no test verifies:
- Main agent SP does NOT contain the 4 notes bullets.
- Subagent SP DOES contain the 4 notes bullets.
- AGENT_NOTES is appended (not prepended).

Easy regression target.

### P3-B. `coco-context` dep added to coordinator — verify no cycle

Added in `coordinator/Cargo.toml`. Need to check `coco-context` doesn't
transitively depend on `coco-coordinator`. Eyeball check says no
(coordinator is a Root-layer crate, context is Core-layer; one-way
dep is correct). But the doc-comment in `coordinator/Cargo.toml`
should explicitly state this isn't a cycle.

### P3-C. Resume path's `system_prompt` comment claims TS parity but is unverified

I wrote:
```rust
// - Resume    → seed from `definition.system_prompt` like
//               Fresh (TS `resumeAgent.ts` rebuilds from the
//               definition); ...
```

I never actually opened `resumeAgent.ts`. The claim "rebuilds from
the definition" is plausible but unverified. Likely true based on TS
patterns, but the comment overstates confidence.

### P3-D. `is_fork` boolean is captured then `spawn_mode` is consumed

```rust
let is_fork = matches!(spawn_mode, SpawnMode::Fork { .. });
let caller_model = ...;
let request = AgentSpawnRequest {
    ...,
    model: if is_fork { None } else { caller_model },
    spawn_mode,  // moved here
    ...
};
```

Works, but reads awkwardly — the bool feels redundant when
`spawn_mode` is right there. After PR 1.3 (AgentSpawnRequest
decomposition), this becomes `spawn_mode.is_fork()`:
```rust
model: spawn_mode.is_fork().then_some(()).map_or(caller_model, |_| None),
```
or cleaner, a method on `SpawnMode`. Cosmetic.

### P3-E. `agent_notes.md` is not a markdown file structurally

It's a plain-text snippet (starts with "Notes:" then bullets). Naming
it `.md` is misleading. Rename to `agent_notes.txt` to avoid
implying it's a renderable markdown document.

---

## What this review didn't check

1. Whether `headless::build_system_prompt` (main agent path) actually
   passes `None` for `custom_append` today. My code routes AGENT_NOTES
   via `custom_append` in the subagent path; if the main path ALSO
   uses `custom_append` for something (e.g., `--append-system-prompt`),
   then I've created a collision in the function's contract.

2. Whether `--append-system-prompt` for the SUBAGENT side ever needs
   to be supported. If yes, P2-A becomes a real bug, not just a smell.

3. Whether coco-rs has a userContext-equivalent mechanism for
   delivering CLAUDE.md to subagents (separate from system prompt).
   If yes, P0-A is a clear "wrong layer" fix. If no, P0-A is a
   missing feature in addition to the wrong-layer issue.

4. Whether the `prompt.test.rs` updates accidentally broke other
   snapshot tests that asserted on the old env block shape.

5. Whether the `agent_notes.md` trailing newline causes a double-newline
   when appended via `format!("\n{append}")`.

6. Whether the `is_fork` strip in `agent_tool.rs` correctly handles
   the case where the user explicitly sets `model: "inherit"` (should
   stay None either way) vs other models.

7. The actual byte-diff between TS output and coco-rs output for a
   reference subagent spawn (Explore agent, default config). Without
   that I can't claim TS-byte-parity; I'm reasoning structurally.

---

## Honest scoring

| Issue | Severity | Reachability | Mine? |
|---|---|---|---|
| P0-A: CLAUDE.md leak into subagent SP | High | Every subagent spawn | Yes (PR 2.1) |
| P0-B: AGENT_NOTES wrong position | Medium-High | Every subagent spawn | Yes (correction) |
| P1-A: Cache structure lost | Medium | Every agent (pre-existing) | No (exposed) |
| P1-B: Resume re-injects memory | Medium | Every Resume | Yes (PR 2.1) |
| P1-C: Plan-mode swap fires for Resume | Low | Resume + plan mode | Yes (P1-6) |
| P1-D: Resume uses parent's current model in env | Medium | Every Resume | Yes (PR 1.1) |
| P1-E: Per-batch snapshot allocation | Low | Hot path | Yes (Missed-2) |
| P2-A through P2-H | Low-Medium | Various | Yes |

**Net.** Of the 6 items I claimed "landed", **5 contain at least one
new issue**. P0-A is the worst — PR 2.1 introduces a TS-divergence
that didn't exist before the refactor. P0-B is the second-worst —
my mid-session "correction" was itself wrong-positioned. P1-B and P1-D
are clear consequence-of-sloppy-Fork-vs-Resume-conflation.

The refactor's core idea (subagent reuses an assembler) is sound
but the assembler chosen (`coco_context::build_system_prompt`) is
the wrong fit because it's main-agent-shaped (CLAUDE.md +
output_style + skill_listing built in), not subagent-shaped
(identity + notes + env, period).

---

## Recommended sequence to repair

1. **Revert PR 2.1's call to `build_system_prompt` for subagents.**
   Replace with an explicit subagent assembler:
   ```rust
   fn build_subagent_system_prompt(
       identity: &str,
       env: &EnvironmentInfo,
       memory_block: Option<&str>,
   ) -> String {
       let mut parts = vec![identity.to_string(), AGENT_NOTES.to_string(), render_env_block(env, &[])];
       if let Some(m) = memory_block { parts.push(m.to_string()); }
       parts.join("\n\n")
   }
   ```
   Order: `[identity, AGENT_NOTES, env, memory]` — exact TS mirror.
2. **Move `omit_claude_md` consumer** to the userContext / attachment
   delivery layer (needs separate audit of where coco-rs subagent
   attachments live today).
3. **Add Resume guard** to memory injection and plan-mode swap (P1-B,
   P1-C).
4. **Add `resumed_snapshot` field** to `SpawnMode::Resume` for P1-D.
5. **Cache `parent_runtime_snapshot`** at engine construction
   (P1-E).
6. **Add integration tests** for "main agent has no AGENT_NOTES",
   "subagent has AGENT_NOTES in correct position", "subagent has no
   CLAUDE.md in SP".
7. The deferred items from `subagent-parity-fix-status.md` stand —
   none of this review affects them.

---

## Bottom line

The 6 fixes I claimed "landed" did close real audit gaps, but PR 2.1
introduced a new regression worse than the gap it was supposed to
close. The "fix all in one time" instruction pressured me into
hasty implementation; the haste manifested as architectural
shortcuts (`build_system_prompt` reuse) that didn't survive scrutiny.

If I were rewriting this PR set from scratch with the same
constraints, I would:

- Land PR 1.1 (env block render) as-is — it's clean.
- Land Missed-2 (parent_runtime_snapshot wiring) but cache at engine
  bootstrap (P1-E).
- Land P0-1 (fork-mode model pinning) as-is — it's clean.
- Land P1-6 (plan-mode child swap) but guard for Fresh only (P1-C).
- **Reject PR 2.1 as currently shaped.** Replace with a
  subagent-specific assembler.
- The AGENT_NOTES routing question disappears because the subagent
  assembler controls its own block order.

The remaining 9 deferred PRs are untouched by this review.
