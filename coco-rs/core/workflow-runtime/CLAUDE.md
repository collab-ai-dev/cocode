# coco-workflow-runtime

Embedded QuickJS (rquickjs) engine executing validated Dynamic Workflow
scripts. Static validation lives in `core/workflow`; the concrete host +
thread plumbing in `core/tools` (`tools/workflow_host.rs`). Design doc:
`docs/internal/workflow-runtime-plan.md` (repo root — `../../../docs/...`
from this crate).

## Threading — the invariant this crate exists for

The engine is **`!Send` and single-threaded by design** (rquickjs
`Ctx`/`Value`). `WorkflowEngine::run` MUST be awaited on a tokio
current-thread runtime inside a `LocalSet` — in practice the host spawns
a dedicated OS thread and bridges subagent spawns back to the main
runtime via its `Handle`. `WorkflowHost` is `#[async_trait(?Send)]` (host
futures are awaited on the engine thread) but the trait object is still
`Send + Sync` so `Arc<dyn WorkflowHost>` can be shared. Awaiting on the
engine thread is what lets `run_nested_workflow` re-enter the `!Send`
engine inline for `workflow()` (same host Arc ⇒ shared semaphore, budget,
journal, abort, agent counter).

## DSL globals contract (`engine.rs`)

- `agent(prompt, opts?)` — async, host-backed. Failure rejects; inside
  `parallel`/`pipeline` a rejected slot becomes `null`.
- `parallel(funcs)` / `pipeline(items, ...stages)` — JS-defined
  combinators over `Promise.allSettled`, `WORKFLOW_ARRAY_CAP` items max;
  concurrency is bounded host-side (each `agent()` waits on a permit).
- `phase(title)` / `log(msg)` — **sync**; `push_progress` must stay
  non-blocking (fire into a channel).
- `workflow(nameOrRef, args?)` — host-backed at depth 0, always throws
  `WORKFLOW_NESTING_LIMIT_ERROR` at depth ≥ 1 (one-level nesting).
- `args` (frozen JSON input) and `budget`
  (`total`/`spent()`/`remaining()`, `Infinity` when no token cap).

## Sandbox (`sandbox.rs`)

`install_sandbox` runs per-context: hardening first, then the determinism
shim. The shim makes `Math.random()`, `Date.now()`, bare `Date()` and
argless `new Date()` throw (they break resume replay); explicit-args
`new Date(...)` stays legal. Hardening deletes ShadowRealm, WebAssembly,
Atomics, SharedArrayBuffer, WeakRef, FinalizationRegistry, queueMicrotask
and freezes `Error.prepareStackTrace`; `eval` is deliberately left intact
(the fixed host surface bounds what a script can do). Defense-in-depth
with the static AST check in `coco_workflow::meta` — neither replaces the
other.

## Run bounds — no whole-run wall clock

`WORKFLOW_SYNC_EVAL_BUDGET` (30 s) caps a single **synchronous** JS chunk
between awaits, NOT the run: the interrupt deadline resets at every
host-callback boundary, so only a runaway sync loop trips it. The run is
instead bounded by `WORKFLOW_AGENT_CAP` lifetime `agent()` calls, the
host token budget (checked *before* each spawn; in-flight agents finish),
the host-side stall watchdog (`WORKFLOW_STALL_MS_DEFAULT` /
`WORKFLOW_STALL_RETRY`), and the `CancellationToken`. 128 MiB memory
limit; `runtime.idle()` flushes tail microtasks regardless of outcome.

## Resume replay (longest-unchanged-prefix)

The journal cache is consulted only while the per-run `diverged` flag is
false; the first miss flips it permanently, so every later `agent()`
re-spawns. `AgentCacheKey` = phase title + verbatim prompt + canonical
opts whitelist (`schema`/`model`/`effort`/`isolation`/`agentType` —
cosmetic opts like `label`/`stall_ms` never change the key). Hashing is
host-side so the engine carries no crypto dep.

## Conventions

`js_to_json`/`json_to_js` bridge through QuickJS's own JSON codec —
`undefined`/functions/symbols become `null` (the DSL's "data is
JSON-serializable" contract); don't hand-roll value walking. Errors are
tier-3 `WorkflowRuntimeError` (Setup/Script/Cancelled); `core/tools`
converts to `ToolError` at the seam.
