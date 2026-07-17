# Dynamic Workflow runtime — execution model & engine plan

> How claude-code actually runs workflow scripts, and what coco-rs needs to
> embed to do the same. Companion to `core/workflow` and
> `core/tools/src/tools/workflow.rs` (the front-half: parse / resolve / launch
> seam). The runtime itself is currently **stubbed** — `WorkflowTool::execute`
> returns an honest "not available" error and registers no task.

## TL;DR

- **The scripts are plain JavaScript, NOT TypeScript.** There is no TS runtime
  and no transpiler (swc/esbuild) anywhere in the path. claude-code parses the
  script with **acorn** in `sourceType: 'script'` mode and *rejects* TS syntax
  outright ("Workflow scripts must be plain JavaScript — TypeScript syntax …
  fails to parse"). The `.ts` extension is cosmetic. coco-rs already mirrors
  this: `core/workflow/src/meta.rs` parses with tree-sitter and rejects TS-only
  constructs.
- **claude-code runs them in Node's built-in `node:vm` module** — it is already
  a Node process, so `new vm.Script(...)` + `vm.runInContext(ctx)` gives it a
  sandboxed JS context for free. No external runtime is installed.
- **coco-rs is a Rust binary; there is no `node:vm`.** To run the scripts it
  must **embed a small JS engine**. Recommendation: **`rquickjs`** (safe Rust
  bindings to QuickJS) — it mirrors claude-code's exact model (small engine +
  sandboxed context + JS-evaluated determinism shim + injected host functions),
  keeps the binary lean, and its async support lets `agent()` await coco's
  `AgentHandle` subagent spawns. **`llrt` / `rustyscript` are the wrong
  dependency** (see comparison).

## 1. How claude-code executes a workflow (the reference)

Source: `analyze/42_workflow/reconstructed_source/runtime.ts`, line-anchored to
`cli_inner_pretty.js`. Five concerns:

1. **Compile** (`compileWorkflowScript`, obf `Mjn` @416383). A `Function("async
   function _check(){…}")` syntax pre-check, then the body is rewritten so every
   `await` flows through a host membrane (`instrumentTopLevelAwait`, obf `B0p` —
   an acorn walk wrapping awaited expressions), then `new vm.Script(program, {
   importModuleDynamically })` — **dynamic `import()` is blocked**.
2. **Sandbox / context** (`buildWorkflowContext`, obf `DWa` @418026). Build the
   VM context, inject the DSL globals + `args` + a **frozen** `budget` +
   `console` (→ `workflow_log` progress) + timers, run the determinism shim, and
   harden intrinsics.
   - **Determinism shim** (`runDeterminismShim`, obf `Djn` @416280):
     `vm.runInContext(DETERMINISM_SHIM, ctx)` makes `Math.random`, `Date.now`,
     and `new Date()` *throw at runtime* — separate from, and in addition to,
     the static AST check (`isNonDeterministic`, the thing coco's `meta.rs`
     already ports).
   - **Intrinsic hardening** (`hardenVMIntrinsics`, obf `KGe` @411340): SES-style
     freeze of core prototype chains + deletion of dangerous globals
     (`ShadowRealm`, `WebAssembly`, `FinalizationRegistry`, `WeakRef`, `Atomics`,
     `SharedArrayBuffer`, `queueMicrotask`, …) + frozen `prepareStackTrace`.
   - **Host↔VM membrane** (`hostToVMClone` obf `vjn`, `readVMError` obf `Hjn`):
     values crossing the boundary are structured-clone deep-copied (functions
     stripped, array sizes capped), so a hostile script can't smuggle a
     non-conforming object or hang the host.
3. **DSL primitives** (`makeWorkflowHooks`, obf `EWa` @416901): `agent()`,
   `parallel()` (a true barrier — calls all thunks then `Promise.allSettled`),
   `pipeline()` (per-item stage flow, no cross-item barrier), `phase()`,
   `log()`, nested `workflow()`. `agent()` → `localExecutor` (obf `U`) →
   `spawnWorkflowAgent` (obf `Tt` @417149): one subagent attempt with stall
   timeout, throttle backoff, and journal cache, mirroring the normal `AgentTool`
   run loop.
4. **Run** (`runWorkflowScript`, obf `MWa` @418079): load the journal, then
   `vm.runInContext` with a **30 s synchronous timeout** (`WORKFLOW_VM_TIMEOUT_MS`,
   obf `Pjn`) raced against the abort signal.
5. **Caps & journal** (constants.ts): agent cap `1000`, stall `180000` ms,
   concurrency `min(16, max(2, cores-2))`, remote default `50`, stall-retry `5`,
   preview `400`. The journal (`journal.jsonl`, SHA-256 key over
   `(phase, prompt, canonical opts)`) drives `resumeFromRunId` replay.

**Why "no TS runtime":** the only language tooling is acorn (parse) +
acorn-walk (the await rewrite and the determinism AST check). The execution
engine is Node's `vm`. Nothing compiles or runs TypeScript.

## 2. What coco-rs needs

coco-rs is a standalone Rust binary, so the `node:vm` shortcut is unavailable.
The runtime must **embed a JS engine** that can:

- compile + run untrusted JS in an **isolated context** (one per workflow run);
- expose **host functions** as globals (`agent`/`parallel`/`pipeline`/…), where
  `agent()` is **async** and bridges into coco's existing subagent spawn path
  (`coco_tool_runtime::AgentHandle` → `app/cli::task_runtime`), driven by tokio;
- **evaluate JS** to install the determinism shim + intrinsic hardening (same
  technique as claude-code — these are JS programs, not engine features);
- enforce a wall-clock **timeout / interrupt** and a **memory cap**;
- (later) support the journal/resume cache and per-agent worktree isolation.

A TypeScript toolchain is explicitly **not** needed (scripts are plain JS, and
TS syntax is rejected at parse time, which `meta.rs` already does).

## 3. Engine options compared

| Crate | Engine | Footprint | Async/host fns | Sandbox/determinism | Verdict |
|---|---|---|---|---|---|
| **`rquickjs`** | QuickJS (C, ~1 MB) | small, fast startup | yes — futures integrate with tokio; closures as globals | context isolation; shim+harden by evaluating JS (exactly like the reference); memory limit + interrupt handler | **Recommended** |
| `boa_engine` | pure-Rust | no C dep, but more crates; heavier compile; ICU if `intl` | host fns yes; async-native fns historically hang-prone, improving (v0.21 JobQueue revamp) | freezing/hardening less battle-tested; 94% test262 | Pick only for browser-wasm / zero-C builds |
| `rustyscript` / `deno_core` | V8 | **huge** (V8 build, long compile, big binary, cross-compile pain) | full event loop | powerful but lock-down + determinism harder; brings a large runtime surface | Overkill; conflicts with lean-binary goal |
| `llrt` | QuickJS (via rquickjs) | a **standalone runtime/binary** for Lambda, not an embedding lib | n/a as a dependency | bundles its own stdlib (fetch/fs/…) we'd have to strip | Wrong layer — depend on `rquickjs` directly, which is what llrt itself uses |
| raw `v8` crate | V8 | huge | low-level | manual | Too low-level |

### `rquickjs` vs `boa_engine` — the realistic finalists

Once V8 and llrt are out, the choice is QuickJS-via-`rquickjs` vs pure-Rust
`boa_engine`. Verified facts (2025/2026):

| Axis | `rquickjs` (QuickJS) | `boa_engine` |
|---|---|---|
| Language coverage | ~full ES2020+ (battle-tested) | 94.12% test262 (v0.21, Oct 2025); edge-case gaps |
| **Async host-fn bridge** (the crux: JS `await agent()` on a tokio future) | **Mature** — "Promises ↔ Rust futures, any executor" | Younger — async-native fns historically hung (issue #3531); JobQueue revamped v0.21 |
| Runtime/binary footprint | **Leaner** — tiny C core, fast startup, ships prebuilt bindings | Heavier — many crates, slower compile, larger (esp. with `intl`/ICU) |
| Build toolchain | C compiler — **already required by coco** (tree-sitter, jemalloc) | Pure Rust, no C |
| wasm | WASI (`wasm32-wasip1/p2`) tested; not browser | **any** rust target incl. `wasm32-unknown-unknown` (browser) |
| Bundled web APIs | **none** (a feature for a deterministic sandbox) | `boa_runtime` adds fetch/setTimeout (would be stripped) |

**Why `rquickjs` wins for coco:** the runtime is fundamentally *async
orchestration*, and the load-bearing integration point is `agent()` = a Rust
tokio future the script `await`s — exactly the reference's `node:vm` model.
`rquickjs` is purpose-built for that; `boa`'s async-host-fn path is the engine's
youngest, historically hang-prone surface. "Scripts are simple" does **not**
de-risk this — it's the host bridge, not the syntax, that's hard. On
*lightweight*, QuickJS is leaner at **runtime + binary** (boa's lightness is
"no C," not "smaller"). On *cross-platform*, boa's only decisive edge is
browser-wasm / zero-C — and coco is a desktop CLI/TUI that **already links C**
(tree-sitter), so `rquickjs` adds no new toolchain burden and still covers WASI.
The sandbox/determinism shim + SES-style hardening also port more reliably onto
QuickJS's fuller spec compliance.

**Pick `boa_engine` instead** only if coco acquires a browser-wasm
(`wasm32-unknown-unknown`) target or adopts a hard no-C / pure-Rust build
policy. Neither holds today.

**Policy note (CLAUDE.md "No unsafe"):** QuickJS is C and `rquickjs` wraps the
FFI. This is consistent with coco's existing posture — the rule is "wrap unsafe
deps in their own crate," and `rquickjs` *is* that wrapper, exactly like
`tree-sitter` (already a `core/workflow` dependency). The unsafe stays
encapsulated behind a safe API; coco code remains safe Rust. Adding the
dependency is a deliberate decision and should be approved before landing.

## 4. Integration design (when greenlit)

The seam already exists and is intentionally dormant:

- `coco_tool_runtime::TaskHandle::register_workflow_task` (impl in
  `app/cli/src/task_runtime/agent.rs`) — persists the script and creates a
  `Running` backgrounded `LocalWorkflow` task. **Today nothing calls it**; the
  stubbed `WorkflowTool::execute` returns an honest error instead of launching.
- `coco_types::WorkflowProgressEvent` / `WorkflowAgentState`
  (`start`/`progress`/`done`/`error` + `cached`) — the progress contract carried
  on `task/progress`, already wired through `tasks::running::emit_task_progress`
  and into the SDK schema.

A real runtime would add a new crate, e.g. **`core/workflow-runtime`**, holding:

1. `WorkflowEngine` — owns an `rquickjs::Runtime` + per-run `Context`; installs
   the determinism shim + hardening; sets a memory limit and an interrupt
   handler bound to the run's `CancellationToken` + a wall-clock deadline.
2. The DSL host functions, each a Rust closure exposed as a context global:
   - `agent(prompt, opts)` → builds an `AgentSpawnRequest` and awaits
     `AgentHandle::spawn` (the same path `AgentTool` uses), applying the caps
     (1000-agent ceiling, `min(16, cores-2)` concurrency, 180 s stall, 5
     retries); streams `WorkflowProgressEvent::WorkflowAgent{…}` deltas.
   - `parallel(thunks)` → `futures::join_all` barrier; `pipeline(items, …stages)`
     → per-item independent stage chains.
   - `phase`/`log` → `WorkflowPhase`/`WorkflowLog` progress events.
   - `workflow(name, args)` → nested run (one level deep).
3. The journal (`journal.jsonl`, SHA-256 key over `(phase, prompt, canonical
   opts)`) for `resumeFromRunId` replay.
4. The driver that `WorkflowTool::execute` calls **instead of** returning the
   stub error: register the task, run the engine on the task-runtime, mark
   terminal. (`is_enabled` flips to default-on and `Feature::Workflow` promotes
   out of `UnderDevelopment` at that point.)

### Effort / phasing

- **P1 (engine spike):** add `rquickjs`; `WorkflowEngine` that runs a trivial
  script with `log()`/`phase()` only + the determinism shim + timeout. Proves
  the embedding, sandbox, and progress seam. (Small.)
- **P2 (agent bridge):** `agent()` → `AgentHandle`, caps, concurrency,
  `parallel()`/`pipeline()`. The bulk of the work — the host↔VM async boundary
  and structured-clone membrane. (Large.)
- **P3 (journal/resume + nested workflow + per-agent worktree).** (Medium.)

Until P1 lands, the honest stub stands: the tool reports the feature is
unavailable rather than faking a launch.
