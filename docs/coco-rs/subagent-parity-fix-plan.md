# Subagent Parity — Comprehensive Fix Plan

Date: 2026-05-16
Supersedes: `subagent-parity-audit.md`, `subagent-parity-audit-review.md`
Constraints: mirror TS, best Rust practice, clear arch, **no back-compat
shims**, **no half-fixes**, optimal solution wins.

This document is the single actionable plan that fixes every gap and
architectural smell identified during the three rounds of audit. It
replaces the prior catalogue with a phased, dependency-ordered
execution plan.

---

## Scope of "fix all"

22 distinct issues, grouped by category:

| Category | Count | Items |
|---|---|---|
| Architecture | 8 | god-function, request bag, serde-skip footgun, non_exhaustive, Vec\<String\>, dual fork-mode decision, filter plan/apply split, load-time memory inject |
| Semantic correctness (P0) | 5 | fork-model cache busting, sync cancel, wildcard memory inject, parent_runtime_snapshot unwired, env block dead-field renders |
| Parity (P1) | 6 | system prompt assembler reuse, AGENT_NOTES injection, omit_claude_md consumer, mcpServers inline, 2-level walk, subagent_type schema, plan-mode child model swap |
| Plumbing | 3 | sync event piping, Resume e2e, output_file divergence |
| Quality | 3 | verbatim builtin prompts, PerCallOverrides inheritance, stale doc cleanup |
| Investigation | 1 | tool result extraction audit |

All are addressed below.

---

## Execution shape

Five phases, ~15 PRs total. Each phase ends in a runnable, testable
state. **Do NOT proceed to a later phase before the earlier phase
lands** — later phases assume earlier types/contracts.

```
Phase 1 ── Foundation: env block + type system
   │
   ├─ Phase 2 ── System prompt reuse + spawn pipeline refactor
   │     │
   │     ├─ Phase 3 ── Fork integrity + cancel propagation
   │     │
   │     └─ Phase 4 ── Plumbing (events, filter, mcp, walkdir, schema)
   │
   └─ Phase 5 ── Polish (verbatim prompts, Resume e2e, cleanup)
```

---

## Phase 1 — Foundation (3 PRs)

### PR 1.1 — Fix `build_system_prompt` env_section + per-model knowledge cutoff

**Why first.** Main agent today renders env block with `{:?}` Debug
formatting, omits `os_version` / `model` / `knowledge_cutoff` / `--add-dir`
even though `EnvironmentInfo` captures them, hardcodes `"May 2025"`,
and never emits the 4 critical AGENT_NOTES (absolute paths, no emojis,
no colon-before-tool-calls, file-path sharing). Subagent reuse later
is meaningless until this is fixed — otherwise we'd propagate the
broken render.

**Files.**
- `coco-rs/core/context/src/environment.rs`
- `coco-rs/core/context/src/prompt.rs`
- `coco-rs/core/context/src/agent_notes.md` (new)
- `coco-rs/app/cli/src/headless.rs` (`build_system_prompt_for_model`)

**Changes.**
1. `Platform::ts_name() -> &str` returns lowercase `"darwin" | "linux"
   | "win32"` matching TS `env.platform`. `ShellKind::ts_name()` returns
   `"zsh" | "bash" | "sh" | "powershell"`.
2. `fn knowledge_cutoff_for_model(model_id: &str) -> Option<&'static str>`
   returns per-model date (mirror TS `getKnowledgeCutoff` exact branches).
   Wire into `get_environment_info`.
3. New `agent_notes.md`: verbatim copy of TS `notes` block from
   `constants/prompts.ts:766-770`. Include via `include_str!`.
4. `build_system_prompt` signature gains
   `additional_working_directories: &[String]`. Refactor env_section
   builder into `fn render_env_block(env, additional_dirs) -> String`
   that produces TS-shape output:
   ```
   Here is useful information about the environment you are running in:
   <env>
   Working directory: <cwd>
   Is directory a git repo: Yes|No
   Additional working directories: a, b           (only if non-empty)
   Platform: darwin
   Shell: zsh (use Unix shell syntax …)           (Windows-only suffix)
   OS Version: Darwin 25.3.0
   </env>
   You are powered by the model <model_id>.
   Assistant knowledge cutoff is <date>.
   ```
5. After env_block, append `AGENT_NOTES` constant (the `include_str!`).
6. `headless::build_system_prompt_for_model` plumbs
   `additional_working_directories` from `Cli.add_dir`. Test fixture
   updated.

**Acceptance.**
- `build_system_prompt` insta snapshot test (new) captures full output
  with all 7 sections present and AGENT_NOTES appended.
- `Platform::ts_name()` tests cover darwin/linux/windows.
- `knowledge_cutoff_for_model` tests cover the 6 TS branches (haiku-4,
  sonnet-4-6, opus-4-6, opus-4-5, opus-4/sonnet-4 fallback, unknown).

---

### PR 1.2 — `ToolAllowList` sum type + `ToolName` for `allowed_tools`

**Why.** `Vec<String>` everywhere allows typos to silently degrade
agents. `Option<bool>` proposals for wildcard tracking are footguns.
A sum type encodes the wildcard/explicit distinction in the type
system.

**Files.**
- `coco-rs/common/types/src/agent.rs` (AgentDefinition.allowed_tools,
  disallowed_tools)
- `coco-rs/core/subagent/src/frontmatter.rs` (parser)
- `coco-rs/core/subagent/src/definition_store.rs`
  (inject_memory_tools)
- `coco-rs/core/subagent/src/filter.rs` (AgentToolFilter)
- `coco-rs/core/subagent/src/prompt.rs` (agent line renderer)
- `coco-rs/core/tools/src/tools/agent/agent_tool.rs` (input schema +
  spawn boundary)
- `coco-rs/core/tool-runtime/src/agent_query.rs`
  (AgentQueryConfig.allowed_tools, disallowed_tools)

**Changes.**
1. New type in `coco-types::agent`:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(tag = "kind", rename_all = "snake_case")]
   pub enum ToolAllowList {
       /// `tools: ['*']` OR `tools:` absent — agent sees every registered tool.
       Wildcard,
       /// `tools: [Read, Write, …]` — explicit allow-list. Empty Vec
       /// means "no tools" (legal but odd).
       Explicit(Vec<ToolName>),
   }
   impl Default for ToolAllowList {
       fn default() -> Self { Self::Wildcard }
   }
   ```
2. `AgentDefinition`:
   ```rust
   pub allowed_tools: ToolAllowList,
   pub disallowed_tools: Vec<ToolName>,
   ```
3. Frontmatter parser parses tool list into `Vec<ToolName>`; unknown
   names → `ValidationError::UnknownTool { name }` (not silently kept).
   `tools: ['*']` → `ToolAllowList::Wildcard`; missing key → also
   Wildcard; explicit list → `Explicit`.
4. `inject_memory_tools` updated:
   ```rust
   fn inject_memory_tools(def: &mut AgentDefinition) {
       let ToolAllowList::Explicit(list) = &mut def.allowed_tools else {
           return;  // Wildcard already covers Read/Write/Edit
       };
       if def.memory_scope.is_none() { return; }
       for t in [ToolName::Read, ToolName::Edit, ToolName::Write] {
           if !list.contains(&t) { list.push(t); }
       }
   }
   ```
   This closes **P0-3 wildcard memory inject** by making the distinction
   `tools: ['*']` vs `tools: [Read]` representable in the type system.
5. `AgentToolFilter::plan` consumes `ToolAllowList` directly. Wildcard
   → no narrowing; Explicit → intersection.
6. MCP tool names (`mcp__server__tool`) get a `ToolName::Mcp { server,
   tool }` variant so `Vec<ToolName>` covers them too. Parser
   recognises the prefix.

**Acceptance.**
- Frontmatter test: `tools: ['Read', 'Write', 'Bogus']` produces an
  error containing "Unknown tool 'Bogus'".
- Memory inject test: `memory: project\ntools: ['*']` leaves
  `allowed_tools = Wildcard` (no injection); `memory: project\ntools:
  ['Bash']` produces `Explicit([Bash, Read, Edit, Write])`.
- Filter test: `ToolAllowList::Wildcard` produces `uses_default_allow_list
  = true`.

---

### PR 1.3 — `AgentSpawnRequest` decomposition + non-optional `ParentRuntimeSnapshot` for fork

**Why.** Closes Missed-2 (`parent_runtime_snapshot` hardcoded `None`),
Arch-B (27-field bag), Arch-C (`#[serde(skip)]` footgun), and sets up
P0-1 fix (fork model pinning) at the type level rather than runtime.

**Files.**
- `coco-rs/core/tool-runtime/src/agent_handle.rs`
- `coco-rs/common/types/src/agent.rs` (ParentRuntimeSnapshot)
- `coco-rs/core/tools/src/tools/agent/agent_tool.rs` (boundary)
- `coco-rs/core/tools/src/handles/` (ToolUseContext gets
  `parent_runtime_snapshot: Option<Arc<ParentRuntimeSnapshot>>`)
- `coco-rs/app/query/src/tool_context.rs` (populate from
  `ApiClient::fingerprint()`)
- `coco-rs/services/inference/src/` (expose `fingerprint()` method
  returning `ParentRuntimeSnapshot`)

**Changes.**
1. Decompose `AgentSpawnRequest`:
   ```rust
   pub struct AgentSpawnRequest {
       pub call: AgentCallInput,            // model-supplied JSON
       pub inheritance: AgentInheritance,   // captured from parent runtime
       pub spawn_mode: SpawnMode,           // fork-aware sum type below
       pub safety: AgentSafetyConfig,       // constraints + can_use_tool
       pub telemetry: AgentTelemetryConfig, // fork_label, skip_transcript
       pub persistence: AgentPersistenceConfig, // session_id, transcript opts
   }

   #[non_exhaustive]  // ← keep only on this OUTER struct (IPC boundary)
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct AgentSpawnRequest { … }
   ```
2. **`SpawnMode` becomes type-safe for fork model pinning**:
   ```rust
   // NOT #[non_exhaustive] — internal enum, compiler must catch new variants.
   #[derive(Debug, Clone)]
   pub enum SpawnMode {
       Fresh {
           model_override: Option<String>,   // request-supplied
       },
       Fork {
           parent_snapshot: Arc<ParentRuntimeSnapshot>,  // NON-OPTIONAL
           rendered_system_prompt: Vec<u8>,
           parent_messages: Vec<serde_json::Value>,
           inherit_tool_pool: bool,
           // Note: no `model_override` field. Fork mode CANNOT override
           // model — it pins to parent_snapshot.model for cache parity.
       },
       Resume {
           parent_messages: Vec<serde_json::Value>,
           parent_snapshot: Arc<ParentRuntimeSnapshot>,
       },
   }
   ```
3. **No `Serialize`/`Deserialize` on `SpawnMode` variants holding
   inheritance.** Wire form is `AgentCallInput` only; runtime form
   carries inheritance. Crossing IPC requires re-derivation at the
   receiving end (panic-on-unwired construction, not silent default).
4. `AgentTool::execute` at the boundary:
   ```rust
   let parent_snapshot = ctx.parent_runtime_snapshot
       .clone()
       .ok_or_else(|| ToolError::ExecutionFailed { … })?;
   let spawn_mode = if call.subagent_type.is_none()
       && coco_subagent::is_fork_subagent_active(&ctx.features, ctx.is_non_interactive)
   {
       // Recursive-fork guard, etc.
       SpawnMode::Fork {
           parent_snapshot,                   // NON-OPTIONAL — closes Missed-2 + P0-1
           rendered_system_prompt: ctx.rendered_system_prompt
               .clone()
               .ok_or(…)?  // Hard fail; no cache parity without it.
               .into_bytes(),
           parent_messages: …,
           inherit_tool_pool: true,
       }
   } else {
       SpawnMode::Fresh {
           model_override: call.model.clone(),
       }
   };
   ```
5. `services/inference::ApiClient::fingerprint() -> ParentRuntimeSnapshot`
   returns `{ provider, api, model_id, base_instructions_hash }`.
   Engine bootstrap populates `ctx.parent_runtime_snapshot` via
   `Arc::new(client.fingerprint())`.

**Acceptance.**
- `cargo check` proves no caller can construct `SpawnMode::Fork`
  without a `ParentRuntimeSnapshot`.
- Test: `AgentTool::execute` with `parent_runtime_snapshot: None` in
  context returns a typed error (not silent fallback).
- Test: fork spawn with `call.model = Some("haiku")` ignores the
  override and uses `parent_snapshot.model_id` (this is mechanically
  true by the type — no separate test for the strip; the test that
  matters is "fork inherits parent model" against the engine).

---

## Phase 2 — System prompt reuse + spawn pipeline (3 PRs)

### PR 2.1 — Subagent reuses `coco_context::build_system_prompt`; `omit_claude_md` becomes live

**Why.** Closes Missed-1 (omit_claude_md dead field), Missed-3
(no priority chain — by reusing the chain that already exists), and
Missed-4 (env details now inherit Phase 1's fix automatically).

**Files.**
- `coco-rs/coordinator/src/agent_handle/spawn.rs`
- (no new files; pure consumer wiring)

**Changes.**
1. Delete the inline `definition_prompt() + memory_block.push_str` path
   at `spawn.rs:680-762`.
2. Replace with a single call to `coco_context::build_system_prompt`:
   ```rust
   let identity = def
       .as_ref()
       .and_then(|d| d.system_prompt.as_deref())
       .unwrap_or(coco_context::DEFAULT_AGENT_IDENTITY);
   let claude_md_files = if def.map(|d| d.omit_claude_md).unwrap_or(false) {
       Vec::new()
   } else {
       coco_context::discover_memory_files(&cwd_path)
   };
   let env_info = coco_context::get_environment_info(&cwd_path, &selection.model);
   let memory_block = if let Some(scope) = def.and_then(|d| d.memory_scope) {
       Some(coco_memory::agent_memory::load_agent_memory_prompt(
           agent_type, scope, &cwd_path, &home,
       ))
   } else { None };
   let system_prompt = coco_context::build_system_prompt(
       identity,
       &claude_md_files,
       &env_info,
       skill_listing.as_deref(),                // from preload step (5.3)
       memory_block.as_deref(),
       request.inheritance.append_system_prompt.as_deref(),
       request.inheritance.output_style.as_ref(),
       &request.inheritance.additional_working_directories,
   ).full_text();
   ```
3. Fork path keeps `rendered_system_prompt` bytes verbatim (already
   correct — parent went through the same builder).
4. Resume path uses `build_system_prompt` with the resumed agent's
   definition.
5. `AgentInheritance` (from PR 1.3) gains
   `append_system_prompt: Option<String>` and `output_style:
   Option<OutputStyleConfig>` — captured from parent's session config
   at the AgentTool boundary.

**Acceptance.**
- Snapshot test: spawn an Explore agent with
  `omit_claude_md: true`; assert `claude_md_files` slice is empty
  going into the builder.
- Snapshot test: spawn a custom agent without `omit_claude_md`;
  assert CLAUDE.md content appears in the rendered prompt.
- Snapshot test: parent runs with `--append-system-prompt "FOO"`;
  spawn a subagent and assert "FOO" appears in the subagent's
  system prompt.

---

### PR 2.2 — `SpawnPipeline` of `SpawnStep`s (refactor the god-function)

**Why.** Closes Arch-A (700-line god-function). Each step becomes
unit-testable in isolation. Sync vs background paths reuse the same
pipeline with different terminal steps.

**Files.**
- `coco-rs/coordinator/src/agent_handle/spawn.rs` → split into:
  - `agent_handle/pipeline/mod.rs` (SpawnPipeline, SpawnContext)
  - `agent_handle/pipeline/steps/{validate, worktree, definition,
    system_prompt, hooks, mcp, skills, fire_start, run_query,
    fire_stop, cleanup_hooks, cleanup_mcp, cleanup_worktree,
    classify_handoff, summarize}.rs`
- `agent_handle/spawn.rs` keeps only the `spawn_subagent` entry that
  builds the right pipeline and runs it.

**Changes.**
1. New trait:
   ```rust
   #[async_trait]
   pub trait SpawnStep: Send + Sync + 'static {
       fn name(&self) -> &'static str;
       async fn run(&self, ctx: &mut SpawnContext) -> Result<(), SpawnError>;
       /// Cleanup that fires whether or not subsequent steps succeeded.
       /// Used for hook deregistration / MCP teardown.
       async fn cleanup(&self, _ctx: &mut SpawnContext) {}
   }
   ```
2. `SpawnContext` holds: `agent_id`, `agent_type`, `request`, `definition`,
   `worktree_session`, `system_prompt`, `selection`, `query_result`,
   etc. — all the loose locals of the current god-function become typed
   fields.
3. `SpawnPipeline::sync()` returns a `Vec<Box<dyn SpawnStep>>` ordered:
   ```
   Validate, AcquireWorktree, ResolveDefinition, AssembleSystemPrompt,
   RegisterFrontmatterHooks, InitPerAgentMcp, PreloadSkills,
   FireSubagentStart, RunQuery, FireSubagentStop, CleanupHooks,
   CleanupMcp, CleanupWorktree, ClassifyHandoff
   ```
4. `SpawnPipeline::background()` swaps `RunQuery` for
   `SpawnBackgroundTask` (registers task, returns AgentSpawnResponse
   with `async_launched`).
5. Each step is < 80 LoC. The whole pipeline is data-driven; adding a
   future concern is a new step, not a new branch in a god-function.

**Acceptance.**
- Each `SpawnStep` has a unit test that runs it against a synthetic
  `SpawnContext` and asserts the relevant field is mutated.
- Integration test: a synthetic pipeline of 3 steps where the middle
  one fails — cleanup of preceding steps fires correctly.
- No file in the new pipeline directory exceeds 200 LoC.

---

### PR 2.3 — Memory injection moves from load-time to spawn-time

**Why.** Closes Arch-H (load-time mutation of definitions). Definition
store stays a pure parser; the `auto_memory_enabled` toggle becomes a
live read at spawn step time.

**Files.**
- `coco-rs/core/subagent/src/definition_store.rs` (remove
  `inject_memory_tools` call from `load`; remove `auto_memory_enabled`
  field on the store)
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/assemble_system_prompt.rs`
  (compute effective allow-list using live `Features`)

**Changes.**
1. Delete `inject_memory_tools` invocation in `definition_store::load`.
   Definitions stay representative of the source `.md`.
2. New helper in `coco-subagent`:
   ```rust
   pub fn effective_allow_list(
       def: &AgentDefinition,
       features: &Features,
   ) -> ToolAllowList {
       let mut list = def.allowed_tools.clone();
       if features.enabled(Feature::AutoMemory) && def.memory_scope.is_some() {
           if let ToolAllowList::Explicit(v) = &mut list {
               for t in [ToolName::Read, ToolName::Edit, ToolName::Write] {
                   if !v.contains(&t) { v.push(t); }
               }
           }
       }
       list
   }
   ```
3. Spawn step `AssembleSystemPrompt` (or a new `ResolveToolFilter`
   step) calls this with the live `Features` from `request.inheritance`.

**Acceptance.**
- Test: load a definition with `memory: project, tools: ['Bash']`,
  call `effective_allow_list(&def, &features_with_auto_memory_off)`
  → returns `Explicit([Bash])` (no injection).
- Test: same definition with auto_memory_enabled → `Explicit([Bash,
  Read, Edit, Write])`.
- Test: hot-reload of `Features::auto_memory` between two spawns of
  the same agent yields different allow-lists.

---

## Phase 3 — Fork integrity + cancel propagation (2 PRs)

### PR 3.1 — `SubagentSelection::for_fork(parent_snapshot)` constructor

**Why.** Closes P0-1 (fork model cache busting) at the type level.
With PR 1.3 in place, `SpawnMode::Fork` already carries a non-optional
`ParentRuntimeSnapshot`. This PR removes the runtime branch in
`resolve_subagent_selection` and replaces with constructors.

**Files.**
- `coco-rs/core/subagent/src/spawn_resolution.rs`
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/resolve_definition.rs`

**Changes.**
1. Delete `resolve_subagent_selection(request_model, def, type_id)`.
   Replace with:
   ```rust
   impl SubagentSelection {
       /// Fork spawns pin to the parent's runtime identity. There is
       /// no caller-supplied model — the type system forbids it.
       pub fn for_fork(parent: &ParentRuntimeSnapshot, def: Option<&AgentDefinition>) -> Self {
           Self {
               model: Some(parent.model_id.clone()),
               model_role: resolve_subagent_role(def, /*type_id*/ None),
               provider_hint: Some(parent.provider.clone()),
           }
       }

       pub fn for_fresh(
           request_model: Option<&str>,
           def: Option<&AgentDefinition>,
           type_id: Option<&AgentTypeId>,
       ) -> Self {
           let model = request_model
               .map(str::to_owned)
               .or_else(|| def.and_then(|d| d.model.clone()))
               .map(resolve_inherit_keyword);  // "inherit" → parent main
           Self {
               model,
               model_role: resolve_subagent_role(def, type_id),
               provider_hint: None,
           }
       }
   }
   ```
2. `"inherit"` keyword resolved at the boundary, not passed downstream
   as a literal string (closes a latent bug from earlier audits).
3. `ResolveDefinition` step matches on `request.spawn_mode` and calls
   the right constructor:
   ```rust
   ctx.selection = match &ctx.request.spawn_mode {
       SpawnMode::Fork { parent_snapshot, .. } | SpawnMode::Resume { parent_snapshot, .. } => {
           SubagentSelection::for_fork(parent_snapshot, ctx.definition.as_deref())
       }
       SpawnMode::Fresh { model_override } => {
           SubagentSelection::for_fresh(model_override.as_deref(), ctx.definition.as_deref(), ctx.agent_type_id.as_ref())
       }
   };
   ```

**Acceptance.**
- Test: `SubagentSelection::for_fork(snapshot, …)` with `snapshot.model_id
  = "claude-haiku-4-5"` returns `model = Some("claude-haiku-4-5")`,
  regardless of `def.model` value.
- Compile test: `SubagentSelection::for_fork` takes no `request_model`
  parameter — proving callers can't supply one.
- Test: `"inherit"` literal never appears in `SubagentSelection.model`
  after `for_fresh`.

---

### PR 3.2 — Cancel token propagation (3-step wiring)

**Why.** Closes P0-2 (sync cancel doesn't propagate). Three steps
because cancel must reach the **child engine's tool execution** layer,
not just the outer `await`.

**Files.**
- `coco-rs/core/tool-runtime/src/agent_handle.rs` (AgentSpawnRequest +
  cancel)
- `coco-rs/core/tool-runtime/src/agent_query.rs` (AgentQueryConfig +
  cancel)
- `coco-rs/core/tools/src/tools/agent/agent_tool.rs` (boundary)
- `coco-rs/app/query/src/agent_adapter.rs` (child engine bootstrap)
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/run_query.rs`

**Changes.**
1. `AgentSpawnRequest` gains
   `cancel: tokio_util::sync::CancellationToken` (NON-optional; default
   to a fresh token in tests).
2. `AgentTool::execute` at the boundary:
   ```rust
   AgentSpawnRequest {
       call,
       inheritance,
       spawn_mode,
       safety,
       telemetry,
       persistence,
       cancel: ctx.cancel.child_token(),   // ← child of parent's tool-level token
   }
   ```
3. `AgentQueryConfig` gains
   `cancel: tokio_util::sync::CancellationToken` (also non-optional).
4. Pipeline step `RunQuery` (sync variant):
   ```rust
   let result = tokio::select! {
       r = engine.execute_query(&ctx.effective_prompt, query_config) => r,
       _ = ctx.request.cancel.cancelled() => {
           Err(QueryError::Cancelled)
       }
   };
   ```
5. Child engine constructed in `agent_adapter` threads
   `config.cancel.child_token()` into every `ToolUseContext.cancel` it
   creates — so the child's tools honor cancel via the same path the
   parent's tools do (`execution.rs:190,310-318`).
6. Background path keeps its own internal token (PR 1.3's
   `AgentTelemetryConfig` carries an opt-out from inheriting parent
   cancel — `inherit_parent_cancel: bool`, default `true` for sync,
   `false` for bg).

**Acceptance.**
- Integration test in `coco-coordinator`: spawn a sync subagent that
  loops in a fake tool; cancel the parent token; assert spawn returns
  `Cancelled` within 100 ms.
- Integration test: spawn a background subagent; cancel parent;
  background spawn continues until its own internal cancellation
  fires (TS parity).
- Test: nested subagents inherit cancel through 2 levels.

---

## Phase 4 — Plumbing (5 PRs)

### PR 4.1 — Sync subagent event piping

**Why.** Closes Missed-6. Sync spawns today emit zero events to the
parent stream — UI shows "Agent tool: in progress" with no nested
detail.

**Files.**
- `coco-rs/core/tool-runtime/src/agent_query.rs` (`event_tx` becomes
  non-optional on `AgentQueryConfig`; tests use `NoOpEventSink`)
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/run_query.rs`
- `coco-rs/app/query/src/agent_adapter.rs`

**Changes.**
1. `AgentQueryConfig.event_tx: Arc<dyn EventSink>` (trait, with
   `NoOpEventSink` for tests).
2. Pipeline step `RunQuery` populates from `ctx.request.event_tx`
   (which `AgentTool::execute` populated from `ctx.event_tx`).
3. Child engine emits `CoreEvent::Stream(StreamEvent::NestedToolCall {
   parent_agent_id, child_agent_id, … })` so the UI can render nested
   tool calls live.

**Acceptance.**
- Test: sync spawn produces ≥ 1 `NestedToolCall` event per tool the
  subagent calls.
- Snapshot test in `app/tui`: nested tool call renders correctly in
  the parent's stream.

---

### PR 4.2 — `ToolRegistry::narrowed_by(plan)` enforcement

**Why.** Closes Arch-G (filter plan computed but applied elsewhere).
Today the plan can be ignored; this PR makes it the only way to
narrow a registry.

**Files.**
- `coco-rs/core/tool-runtime/src/registry.rs`
- `coco-rs/core/subagent/src/filter.rs` (visibility tweaks)
- `coco-rs/app/query/src/agent_adapter.rs` (consumer)

**Changes.**
1. `ToolFilterPlan.allowed_tools` becomes `pub(crate)` within
   `coco-subagent`. External callers can only inspect via
   `ToolRegistry::narrowed_by`.
2. New method:
   ```rust
   impl ToolRegistry {
       pub fn narrowed_by(&self, plan: &ToolFilterPlan) -> Self { … }
   }
   ```
3. `agent_adapter::build_child_registry` calls `narrowed_by` exclusively.
4. The "unknown tools" diagnostic (`plan.unknown_tools`) gets logged
   at the spawn step boundary so users see typo warnings.

**Acceptance.**
- Test: `registry.narrowed_by(&plan)` produces a registry whose
  `tool_names()` is a subset of plan.allowed_tools and the parent
  registry.
- Test: unknown tools in plan log a warning event.

---

### PR 4.3 — `mcpServers` inline form + `walkdir` 2-level discovery

**Why.** Closes P1-3 (mcpServers inline form) and P1-4 (1-level walk —
reclassified as P0 because agents in subfolders silently disappear).

**Files.**
- `coco-rs/core/subagent/Cargo.toml` (add `walkdir`)
- `coco-rs/core/subagent/src/frontmatter.rs` (parse inline form)
- `coco-rs/common/types/src/agent.rs` (`McpServerConfig` enum)
- `coco-rs/core/subagent/src/definition_store.rs` (walkdir)
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/init_mcp.rs`
  (register inline servers)

**Changes.**
1. New type:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(untagged)]
   pub enum McpServerConfig {
       /// String-ref: `mcpServers: [github, linear]`
       Reference(String),
       /// Inline: `mcpServers: [{ slack: { command: "/path", args: [] } }]`
       Inline { name: String, config: McpInlineConfig },
   }
   ```
2. Frontmatter parser accepts both forms; populates
   `AgentDefinition.mcp_servers: Vec<McpServerConfig>`.
3. `definition_store::sorted_md_paths`: replace `read_dir` with
   `WalkDir::new(dir).max_depth(2).follow_links(false)`. 1 MiB size cap
   enforced inside the walker via metadata check.
4. Pipeline step `InitPerAgentMcp` registers inline servers via the
   existing dynamic-MCP path; teardown at cleanup.

**Acceptance.**
- Test: parse a markdown agent with inline `mcpServers: [{ slack: {
  command: '…' } }]`; assert one `McpServerConfig::Inline` element.
- Test: place an agent at `.claude/agents/refactor/explore.md`;
  assert `AgentDefinitionStore::load` discovers it.
- Test: place a 2 MB markdown file; assert it's skipped, not panicked
  on.
- Integration test: spawn an agent with inline MCP; assert the server
  is connected before `RunQuery` starts.

---

### PR 4.4 — `subagent_type` dynamic schema + plan-mode child model swap

**Why.** Closes P1-5 (subagent_type schema enum) and P1-6 (plan-mode
subagent client swap).

**Files.**
- `coco-rs/core/tools/src/tools/agent/agent_tool.rs` (per-turn
  schema rebuild)
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/resolve_definition.rs`
- `coco-rs/app/query/src/agent_adapter.rs` (engine factory plan-mode
  swap)

**Changes.**
1. `AgentTool::input_schema` becomes turn-aware: when called with a
   live `AgentCatalogSnapshot`, the `subagent_type` field is an enum
   of the catalog's active agent types. Reconstructed each turn (cheap
   — schemas are small).
2. `ResolveDefinition` step: if `request.call.subagent_type` is set
   but not in the catalog, return `SpawnError::UnknownAgentType {
   requested, available }`. No silent fallback to `general-purpose`.
3. In `agent_adapter::child_engine_for`: when
   `query_config.permission_mode == Some("plan")`, force
   `model_role = ModelRole::Plan` regardless of selection's role.

**Acceptance.**
- Test: schema for AgentTool with catalog of 5 agents lists all 5
  agent types in the JSON Schema enum.
- Test: `subagent_type: "Bogus"` returns `UnknownAgentType` with
  available list.
- Test: plan-mode spawn of a custom agent routes through `ModelRole::Plan`
  in the child engine.

---

### PR 4.5 — Two parallel fork-mode decisions collapsed

**Why.** Closes Arch-F. Today `is_fork_subagent_active(features,
non_interactive)` and `SpawnMode::Fork` can drift.

**Files.**
- `coco-rs/core/subagent/src/fork.rs` (`SpawnMode::Fork` construction
  becomes `pub(crate)` to `coco-subagent`)
- `coco-rs/core/tool-runtime/src/agent_handle.rs` (`SpawnMode::Fork`
  variant is constructed only by `coco_subagent::try_fork(...)`)
- `coco-rs/core/tools/src/tools/agent/agent_tool.rs` (use try_fork)

**Changes.**
1. `pub fn try_fork(features, is_non_interactive, parent_snapshot,
   rendered_system_prompt, parent_messages) -> Option<SpawnMode>`
   returns `Some(Fork{…})` only when the gate passes AND inputs are
   valid; `None` otherwise.
2. `SpawnMode::Fork` fields are `pub(crate)` to `coco-subagent`; no
   one outside that crate can construct it.
3. `AgentTool::execute` calls `try_fork(...)` directly, falls through
   to `Fresh` on `None`.

**Acceptance.**
- Compile test: external crate cannot construct
  `SpawnMode::Fork { … }` directly.
- Test: `try_fork` returns `None` when
  `is_fork_subagent_active(features, true) == false`.

---

## Phase 5 — Polish (3 PRs)

### PR 5.1 — Verbatim builtin prompts via `include_str!`

**Why.** Closes P2-1. Today `builtin_prompts.rs` is hand-written Rust
prose paraphrasing TS. For eval / behavioral diffing this is a parity
contract violation.

**Files.**
- `coco-rs/core/subagent/src/builtin_prompts.rs`
- `coco-rs/core/subagent/src/ts_prompts/{general_purpose, explore, plan,
  verification, coco_guide, statusline}.md` (new, verbatim TS content)

**Changes.**
1. Each builtin's `system_prompt` body comes from
   `include_str!("ts_prompts/<name>.md")`.
2. Each `.md` file copies the TS string from `built-in/<agent>.ts`
   verbatim, with a header comment `# Source: tools/AgentTool/built-in/<file>.ts`.
3. Same for `whenToUse` strings — extract to `<name>_when.txt` files
   and `include_str!`.

**Acceptance.**
- Snapshot test per builtin: prompt body byte-equals TS source after
  whitespace normalization.
- Doc test: every `.md` file under `ts_prompts/` has a `# Source:`
  header pointing to a real TS file.

---

### PR 5.2 — Resume path end-to-end test + transcript symlink decision

**Why.** Closes Missed-7 (Resume not exercised e2e) and Missed-8
(output_file divergence).

**Files.**
- `coco-rs/coordinator/tests/resume_e2e.rs` (new)
- `coco-rs/coordinator/CLAUDE.md` (decision documented)

**Changes.**
1. Decision (per "optimal solution wins"): **restore TS symlink
   convention** so `TaskOutput` and any TS-aligned tooling sees the
   subagent's output by tailing the JSONL transcript. The current
   divergence is convenient for coco-rs UI but breaks user workflows
   that depend on TS format.
2. `SwarmAgentHandle::spawn_background` writes the JSONL transcript
   AND creates a symlink `<sessions_dir>/<session>/output/<agent_id>.output
   → <transcript>.jsonl`.
3. E2E test:
   ```
   1. Spawn a background subagent that runs 3 tools.
   2. Capture the agent_id.
   3. Kill the spawn task (simulate process restart).
   4. Resume via SwarmAgentHandle::resume_background(agent_id).
   5. Assert the resumed conversation contains all 3 prior tool uses.
   ```

**Acceptance.**
- E2E test passes.
- `coordinator/CLAUDE.md` "Open follow-ups" section removes the
  symlink divergence line; adds the new behavior.

---

### PR 5.3 — Tool result extraction audit + PerCallOverrides inheritance + stale doc cleanup

**Why.** Closes Missed-9, Missed-10, P2-2 (extra_allow_list), and
flushes stale T5/T6/T7 markers.

**Files.**
- `coco-rs/coordinator/src/agent_handle/pipeline/steps/run_query.rs`
- `coco-rs/core/tool-runtime/src/agent_handle.rs`
  (`AgentInheritance.per_call_overrides`)
- `coco-rs/core/subagent/CLAUDE.md` (delete "Known Phase-1 Gaps")
- `coco-rs/core/subagent/src/filter.rs` (delete or wire
  `extra_allow_list`)
- `coco-rs/core/tools/src/tools/agent/agent_tool.rs` (delete
  T5/T6/T7 comments — either fixed by prior PRs or filed as issues)

**Changes.**
1. **Result extraction.** Audit and document:
   - `RunQuery` step extracts `QueryResult.messages` → final assistant
     text via a new `extract_final_assistant_text(&[Message]) ->
     Option<String>` function.
   - Thinking blocks stripped; tool_use blocks omitted; multiple text
     blocks concatenated with `\n\n`.
   - Empty result → `EMPTY_AGENT_OUTPUT_MARKER`.
   - Test coverage: each branch (empty, single-text, multi-text,
     thinking-only, tool-use-only).
2. **PerCallOverrides.** Decision: subagents inherit parent's overrides
   unless their definition declares its own. Add to `AgentInheritance`:
   ```rust
   pub per_call_overrides: Option<Arc<PerCallOverrides>>,
   ```
3. **extra_allow_list.** Either wire to Skill-Command path (Phase 8
   per the audit) or delete. Recommend delete — it's dead today and
   the skill path can be added when Phase 8 lands.
4. **Stale docs.**
   - Delete "Known Phase-1 Gaps" section in
     `core/subagent/CLAUDE.md`.
   - Delete `agent_spawn`/`agent_advanced` comment in
     `app/cli/src/paths.rs`.
   - T5/T6/T7 comments in `agent_tool.rs` — for each: if fixed by a
     prior PR, delete; if not yet fixed, file a GitHub issue and
     reference the issue number in the comment.

**Acceptance.**
- All TODO/T5/T6/T7 markers in the subagent area resolved or linked
  to issues.
- `extra_allow_list` field deleted (or wired with a test).
- `core/subagent/CLAUDE.md` reflects actual state.

---

## Cross-cutting: tests, docs, telemetry

### Integration test matrix (added incrementally per PR)

Each integration test runs against a real `AgentQueryEngine`
implementation (the `coco-query` adapter). Located in
`coco-rs/coordinator/tests/`.

| Test | Verifies | PR |
|---|---|---|
| `env_block_parity` | Rust env block matches TS structure byte-for-byte after whitespace normalization | 1.1 |
| `wildcard_memory_inject` | `tools: ['*']` + `memory: project` produces Wildcard allow-list at runtime, but child can write to memory dir | 1.2, 2.3 |
| `fork_pins_to_parent_model` | Fork spawn with `model: 'haiku'` in input still uses parent's model | 3.1 |
| `sync_cancel_propagates` | Parent token cancel terminates a sync subagent's tool call within 100 ms | 3.2 |
| `bg_cancel_independent` | Parent cancel does NOT terminate a background subagent | 3.2 |
| `omit_claude_md_works` | Explore agent's prompt does NOT contain project CLAUDE.md content | 2.1 |
| `append_system_prompt_inherited` | `--append-system-prompt "FOO"` appears in subagent prompt | 2.1 |
| `nested_tool_call_events` | Sync spawn emits ≥ 1 NestedToolCall event per child tool | 4.1 |
| `unknown_subagent_type` | `subagent_type: "Bogus"` returns typed error, not silent fallback | 4.4 |
| `plan_mode_child_uses_plan_role` | Custom agent with `mode: 'plan'` routes through Plan role | 4.4 |
| `subfolder_agents_discovered` | `.claude/agents/refactor/explore.md` appears in catalog | 4.3 |
| `inline_mcp_server_connects` | Inline `mcpServers` config registers + tears down | 4.3 |
| `resume_after_kill` | Background spawn JSONL → resume → conversation intact | 5.2 |
| `builtin_prompt_verbatim` | Each builtin's `system_prompt` byte-matches TS source | 5.1 |

### Documentation updates per PR

Every PR updates the relevant `CLAUDE.md`:
- Core changes → `core/subagent/CLAUDE.md` + `core/tool-runtime/CLAUDE.md`
- Pipeline → `coordinator/CLAUDE.md`
- Prompt assembly → `core/context/CLAUDE.md`
- Audit-gaps reference list (`docs/coco-rs/audit-gaps.md`): mark
  resolved items per PR.

### Telemetry events per PR

New events to add as the work lands:
- `subagent_spawn_cancelled` (cancel propagation telemetry)
- `subagent_fork_cache_pinned` (proves parent_snapshot was used)
- `subagent_unknown_type_rejected` (P1-5 enforcement)
- `subagent_walked_subdir` (nested discovery, debug level)

---

## Dependency graph

```
                    PR 1.1 (env block)
                          ↓
                    PR 2.1 (subagent reuses build_system_prompt)
                          ↑
                    PR 1.2 (ToolAllowList + ToolName)
                          ↓
        ┌─────────────────┼─────────────────┐
        ↓                 ↓                 ↓
   PR 2.3 (memory     PR 4.2 (filter    PR 4.3 (walkdir +
   spawn-time)        enforce)          inline mcp)
        ↑
   PR 2.2 (SpawnPipeline) ←── PR 1.3 (AgentSpawnRequest decomp)
        ↓                              ↓
   PR 3.2 (cancel)            PR 3.1 (SubagentSelection::for_fork)
        ↑                              ↓
   PR 4.1 (sync events)        PR 4.5 (collapse fork decisions)
        ↑
   PR 4.4 (schema + plan-mode)
        ↓
   PR 5.1 (verbatim prompts)
   PR 5.2 (resume e2e)
   PR 5.3 (extraction + cleanup)
```

**Critical path:** 1.1 → 1.2 → 1.3 → 2.1/2.2 → 3.1/3.2 → 5.*

**Parallelizable:** 1.1 and 1.2 can land independently. 4.2, 4.3, 4.5
can land in parallel once 1.2/1.3 are in.

---

## What this plan does NOT do

Honest list of out-of-scope items (consciously deferred):

1. **`ToolCallRunner` refactor.** The eager-streaming safety
   invariant violation noted in `agent-loop-refactor-plan.md` is real
   but orthogonal to subagent parity. Separate effort.
2. **Workflow tool registration.** TS has it; Rust doesn't. Wait
   until `Workflow` ships; the filter constants are correct for the
   "no Workflow" world.
3. **`USER_TYPE=ant` build path.** Project's non-goals explicitly
   exclude ant cloud routes. The Agent-in-deny-list divergence stays.
4. **Handoff classifier orchestration.** Logic exists; turning it on
   needs product decision (paid LLM call gating). Code-side work is
   done.
5. **OTel L2/L4/L5/L6 spans for spawn pipeline.** Pipeline refactor
   (PR 2.2) makes adding spans trivial; do it as a follow-up after
   pipeline lands so spans match the final step layout.
6. **Forking from teammates (cross-process pane subagents).** The
   in-process AgentTool path is covered; cross-pane fork is rarer
   and the existing teammate path handles it via a different code
   route.

---

## Success criteria

The plan is "done" when:

1. All 15 PRs land.
2. The integration test matrix (14 tests) all pass.
3. `docs/coco-rs/subagent-parity-audit.md` is deleted (replaced by
   this doc + the actual code).
4. `core/subagent/CLAUDE.md` "Known Phase-1 Gaps" section is empty.
5. A new audit run finds no P0 or P1 items in subagent area.

The plan is **not** "done" when only individual PRs land. The
architecture changes (PR 1.3, 2.2, 2.3, 3.1, 4.5) are mutually
reinforcing — landing the type changes without the pipeline refactor
leaves smell intact; landing the pipeline without the type changes
papers over the seams.

---

## Sizing

Rough estimates per PR (LoC and review burden):

| PR | LoC change | Review difficulty | Risk |
|---|---|---|---|
| 1.1 env block | ~200 | Low | Low (additive) |
| 1.2 ToolAllowList | ~600 | Medium | Medium (touches every filter site) |
| 1.3 SpawnRequest decomp | ~800 | High | High (touches IPC schema) |
| 2.1 system prompt reuse | ~150 | Low | Low |
| 2.2 SpawnPipeline | ~1200 | High | Medium (refactor, no behavior change) |
| 2.3 memory at spawn | ~150 | Low | Low |
| 3.1 SubagentSelection ctors | ~200 | Medium | Low |
| 3.2 cancel propagation | ~400 | Medium | Medium |
| 4.1 sync events | ~300 | Medium | Low |
| 4.2 narrowed_by | ~250 | Medium | Low |
| 4.3 walkdir + inline mcp | ~400 | Medium | Low |
| 4.4 schema + plan-mode | ~250 | Medium | Low |
| 4.5 fork decisions | ~150 | Low | Low |
| 5.1 verbatim prompts | ~600 (mostly md files) | Low | Low |
| 5.2 resume e2e | ~400 | Medium | Medium |
| 5.3 cleanup | ~300 | Low | Low |

**Total:** ~6300 LoC across 15 PRs. Spread over 3-4 weeks of focused
work assuming review velocity.

The two big PRs (1.3 and 2.2) are the architecture core. Everything
else is downstream consequence. If only 2 PRs of this plan could
land, they should be 1.3 and 2.2 — because they enable the rest as
small mechanical changes rather than parallel refactors.

---

## How this differs from the prior audit

The prior `subagent-parity-audit.md` listed 13 fixes as
roughly-flat items. This plan reframes:

- **5 are now architecture changes** (PRs 1.2, 1.3, 2.2, 2.3, 4.5)
  that close 8 architectural smells and unblock the rest.
- **3 are now consequences** of the architecture (PRs 3.1, 3.2, 4.2
  become small mechanical changes once 1.2/1.3 land).
- **2 are completely subsumed** by reusing existing code (Missed-1,
  Missed-3 collapse into PR 2.1).
- **2 are deleted** as out-of-scope (Workflow tool, ant build).

The headline: "13 fixes" was wrong shape. The honest shape is
**"5 architecture changes + 10 mechanical fixes that exist only
because the architecture wasn't right."** Doing them in that order is
the optimal solution.
