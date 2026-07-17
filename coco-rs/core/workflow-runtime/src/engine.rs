//! The workflow execution engine: a sandboxed QuickJS context running the
//! script body wrapped in an async IIFE, with the DSL globals
//! (`agent`/`parallel`/`pipeline`/`phase`/`log`/`workflow` + `args`/`budget`)
//! bridged to a [`WorkflowHost`]. Single-threaded by design (rquickjs `Ctx`/
//! `Value` are `!Send`) — drive it on a tokio current-thread runtime /
//! `LocalSet`. See `docs/internal/workflow-runtime-plan.md`.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use coco_types::WorkflowAgentState;
use coco_types::WorkflowProgressEvent;
use rquickjs::AsyncContext;
use rquickjs::AsyncRuntime;
use rquickjs::CatchResultExt;
use rquickjs::Ctx;
use rquickjs::Function;
use rquickjs::IntoJs;
use rquickjs::Object;
use rquickjs::Promise;
use rquickjs::Value;
use rquickjs::prelude::Async;
use rquickjs::prelude::Func;
use rquickjs::prelude::Opt;
use tokio_util::sync::CancellationToken;

use crate::convert::js_to_json;
use crate::convert::json_to_js;
use crate::error::CancelledSnafu;
use crate::error::ScriptSnafu;
use crate::error::SetupSnafu;
use crate::error::WorkflowRuntimeError;
use crate::host::WorkflowAgentOpts;
use crate::host::WorkflowHost;

/// Cap on a single *synchronous* JS-evaluation chunk between `await`
/// boundaries (CC's `vm.runInContext({timeout})` ceiling — 30s). This is NOT a
/// whole-run wall clock: a real multi-agent workflow runs for minutes. The
/// sync-budget instant is reset each time control returns from a host callback
/// (`agent`/`log`/`phase`/`budget`) into JS, so only a runaway synchronous loop
/// — not legitimate long-running async orchestration — trips the interrupt
/// handler. The whole run is instead bounded by the agent cap, the per-agent
/// stall watchdog, the token budget, and explicit cancellation.
pub const WORKFLOW_SYNC_EVAL_BUDGET: Duration = Duration::from_secs(30);

/// Lifetime cap on the number of `agent()` calls a single workflow run may make.
/// Backstops unbounded `while (budget.remaining() > 0)` loops when no token
/// budget is set; exceeding it throws in `agent()`.
pub const WORKFLOW_AGENT_CAP: i32 = 1000;

/// Maximum number of items a single `parallel()`/`pipeline()` call accepts.
pub const WORKFLOW_ARRAY_CAP: usize = 4096;

/// Error thrown by a CHILD workflow's `workflow()` (CC's one-level guard). A
/// parent workflow may call `workflow(nameOrRef, args)` to run a saved/child
/// workflow inline, but that child cannot itself nest further — nesting is
/// limited to one level.
pub const WORKFLOW_NESTING_LIMIT_ERROR: &str =
    "workflow() cannot be called from within a child workflow — nesting is limited to one level.";

/// JS-defined DSL combinators + `budget`, evaluated at context init. `parallel`
/// is a true barrier (`Promise.allSettled`); `pipeline` flows each item through
/// all stages independently. The array-length guard mirrors CC's per-call
/// `WORKFLOW_ARRAY_CAP`; concurrency is bounded host-side (each `agent()` waits
/// on a permit), so both combinators still fire every thunk. `workflow()` is NOT
/// installed here — it is host-backed and depth-aware, installed in
/// [`install_globals`].
fn dsl_combinators() -> String {
    format!(
        r#"(() => {{
  const ARRAY_CAP = {WORKFLOW_ARRAY_CAP};
  globalThis.parallel = async function parallel(funcs) {{
    if (!Array.isArray(funcs)) throw new TypeError('parallel() expects an array of functions');
    if (funcs.length > ARRAY_CAP) throw new RangeError('parallel()/pipeline() accepts at most ' + ARRAY_CAP + ' items.');
    for (const f of funcs)
      if (typeof f !== 'function')
        throw new TypeError('parallel() expects functions. Wrap each call: () => agent(...)');
    const settled = await Promise.allSettled(funcs.map(f => f()));
    return settled.map(s => s.status === 'fulfilled' ? s.value : null);
  }};
  globalThis.pipeline = async function pipeline(items, ...stages) {{
    if (!Array.isArray(items)) throw new TypeError('pipeline() expects an array as its first argument');
    if (items.length > ARRAY_CAP) throw new RangeError('parallel()/pipeline() accepts at most ' + ARRAY_CAP + ' items.');
    const settled = await Promise.allSettled(items.map(async (item, i) => {{
      let value = await item;
      for (const stage of stages) {{
        if (value === null) break;
        value = await stage(value, item, i);
      }}
      return value;
    }}));
    return settled.map(s => s.status === 'fulfilled' ? s.value : null);
  }};
}})()"#
    )
}

/// The workflow execution engine.
pub struct WorkflowEngine;

impl WorkflowEngine {
    /// Run `script` to completion, returning its resolved JSON value. `args` is
    /// exposed to the script as the global `args`; `host` backs `agent()` and
    /// progress. `cancel` cancels the whole run; `sync_eval_budget` caps a
    /// single *synchronous* JS-evaluation chunk between `await` boundaries (NOT
    /// a whole-run wall clock — see [`WORKFLOW_SYNC_EVAL_BUDGET`]). The run
    /// itself is bounded by the agent cap, the per-agent stall watchdog, the
    /// token budget, and `cancel`.
    ///
    /// `depth` is the nesting level: a top-level run is `0` (its `workflow()`
    /// global is host-backed and callable); a child workflow re-entered via
    /// [`WorkflowHost::run_nested_workflow`] runs at `depth >= 1`, where
    /// `workflow()` always throws [`WORKFLOW_NESTING_LIMIT_ERROR`] (the one-level
    /// guard). Re-entering with the SAME `host` Arc is what shares governance.
    ///
    /// MUST be awaited on a tokio current-thread runtime inside a `LocalSet`
    /// (the engine is `!Send`).
    pub async fn run(
        script: String,
        args: serde_json::Value,
        host: Arc<dyn WorkflowHost>,
        cancel: CancellationToken,
        sync_eval_budget: Duration,
        depth: i32,
    ) -> Result<serde_json::Value, WorkflowRuntimeError> {
        let runtime = AsyncRuntime::new().map_err(setup_err)?;
        let context = AsyncContext::full(&runtime).await.map_err(setup_err)?;

        // Sync-eval guard: the interrupt handler fires when the *current*
        // synchronous chunk overruns `sync_eval_budget` or the run is
        // cancelled. The deadline is reset on every host-callback boundary
        // (start of `agent`/`log`/`phase`/`budget`), so returning from an
        // `await` into JS always begins a fresh chunk — a minutes-long async
        // workflow never trips it, only a runaway synchronous loop does.
        let sync_deadline = Rc::new(Cell::new(Instant::now() + sync_eval_budget));
        runtime.set_memory_limit(128 * 1024 * 1024).await;
        {
            let cancel = cancel.clone();
            let sync_deadline = sync_deadline.clone();
            runtime
                .set_interrupt_handler(Some(Box::new(move || {
                    Instant::now() >= sync_deadline.get() || cancel.is_cancelled()
                })))
                .await;
        }

        // Setup: sandbox + DSL combinators + host-backed globals. Sync block.
        let setup_host = host.clone();
        let setup_args = args.clone();
        let setup_sync_deadline = SyncBudget {
            deadline: sync_deadline.clone(),
            budget: sync_eval_budget,
        };
        context
            .with(move |ctx| -> rquickjs::Result<()> {
                crate::install_sandbox(&ctx)?;
                ctx.eval::<(), _>(dsl_combinators())?;
                install_globals(&ctx, setup_host, &setup_args, setup_sync_deadline, depth)?;
                Ok(())
            })
            .await
            .map_err(|e| {
                SetupSnafu {
                    message: e.to_string(),
                }
                .build()
            })?;

        // Run the body wrapped in an async IIFE so top-level `await`/`return`
        // work and the result surfaces as a Promise we await to completion.
        let wrapped = format!("(async () => {{\n{script}\n}})()");
        // An `async` closure (stabilized) lets `async_with` resolve the HRTB
        // context lifetime that a plain `|ctx| async move { .. }` cannot.
        let run_fut = context.async_with(async move |ctx| {
            let promise = match ctx.eval::<Promise, _>(wrapped).catch(&ctx) {
                Ok(promise) => promise,
                Err(error) => return Err(script_err(error)),
            };
            let value = match promise.into_future::<Value>().await.catch(&ctx) {
                Ok(value) => value,
                Err(error) => return Err(script_err(error)),
            };
            js_to_json(&ctx, value).map_err(script_err)
        });

        // No whole-run wall-clock kill: a real multi-agent workflow legitimately
        // runs for minutes. The run ends when the script resolves, when the
        // sync-eval guard interrupts a runaway synchronous chunk (surfaced as a
        // script error), or on explicit cancellation. Per-agent stalls are
        // reclaimed by the host-side watchdog, not by killing the whole run.
        let outcome = tokio::select! {
            r = run_fut => r,
            _ = cancel.cancelled() => Err(CancelledSnafu.build()),
        };

        // Flush tail microtasks / detached host futures regardless of outcome.
        runtime.idle().await;
        outcome
    }
}

fn setup_err(e: rquickjs::Error) -> WorkflowRuntimeError {
    SetupSnafu {
        message: e.to_string(),
    }
    .build()
}

fn script_err<E: std::fmt::Display>(e: E) -> WorkflowRuntimeError {
    ScriptSnafu {
        message: e.to_string(),
    }
    .build()
}

/// Shared handle for resetting the sync-eval deadline at host-callback
/// boundaries. Each host global (`agent`/`log`/`phase`/`budget`) calls
/// [`SyncBudget::reset`] on entry so the chunk of synchronous JS that follows
/// the await gets a fresh [`WORKFLOW_SYNC_EVAL_BUDGET`] window. `Rc`/`Cell`
/// because every consumer runs on the single engine thread.
#[derive(Clone)]
struct SyncBudget {
    deadline: Rc<Cell<Instant>>,
    budget: Duration,
}

impl SyncBudget {
    fn reset(&self) {
        self.deadline.set(Instant::now() + self.budget);
    }
}

/// Install the host-backed globals: `agent` (async), `phase`/`log` (sync),
/// `args` (frozen JSON), `budget` (frozen), and `workflow` (async, depth-aware:
/// host-backed at depth 0, throwing the one-level guard at depth >= 1).
fn install_globals<'js>(
    ctx: &Ctx<'js>,
    host: Arc<dyn WorkflowHost>,
    args: &serde_json::Value,
    sync_budget: SyncBudget,
    depth: i32,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let agent_index = Rc::new(Cell::new(0i32));
    let phase_index = Rc::new(Cell::new(0i32));
    // Resume replay cursor: the cache is consulted only while `diverged` is
    // false. The first cache miss flips this permanently, so every subsequent
    // `agent()` call re-spawns — the longest-unchanged-prefix guarantee (CC's
    // `{cursor, diverged}` in `journal.ts`). Single-threaded engine ⇒ `Cell`.
    let diverged = Rc::new(Cell::new(false));

    // args — the workflow input value, frozen.
    globals.set("args", json_to_js(ctx, args)?)?;

    // budget — frozen object with live spent/remaining views backed by the
    // host. `remaining()` stays Infinity only when no token cap is known.
    {
        let total = host.budget_total_tokens();
        globals.set("__cocoWorkflowBudgetTotal", Func::from(move || total))?;
        let host_for_spent = host.clone();
        globals.set(
            "__cocoWorkflowBudgetSpent",
            Func::from(move || host_for_spent.budget_spent_tokens()),
        )?;
        ctx.eval::<(), _>(
            r#"globalThis.budget = Object.freeze({
              get total() { return globalThis.__cocoWorkflowBudgetTotal(); },
              spent() { return globalThis.__cocoWorkflowBudgetSpent(); },
              remaining() {
                const total = globalThis.__cocoWorkflowBudgetTotal();
                if (total === null || total === undefined) return Infinity;
                return Math.max(0, total - globalThis.__cocoWorkflowBudgetSpent());
              }
            });"#,
        )?;
    }

    // log(message) — sync.
    {
        let host = host.clone();
        let sync_budget = sync_budget.clone();
        globals.set(
            "log",
            Func::from(move |message: String| {
                sync_budget.reset();
                host.push_progress(WorkflowProgressEvent::WorkflowLog { message });
            }),
        )?;
    }

    // phase(title) — sync; assigns the next phase index.
    {
        let host = host.clone();
        let sync_budget = sync_budget.clone();
        globals.set(
            "phase",
            Func::from(move |title: String| {
                sync_budget.reset();
                let index = phase_index.get();
                phase_index.set(index + 1);
                host.push_progress(WorkflowProgressEvent::WorkflowPhase { index, title });
            }),
        )?;
    }

    // Clones reserved for the `workflow()` global below: the `agent` block moves
    // `host` and `sync_budget`, so capture what the nested-workflow global needs
    // first.
    let workflow_host = host.clone();
    let workflow_sync_budget = sync_budget.clone();

    // agent(prompt, opts?) — async; spawns one subagent and awaits its result.
    {
        let agent_sync_budget = sync_budget;
        let agent = move |ctx: Ctx<'js>, prompt: String, opts: Opt<Object<'js>>| {
            let host = host.clone();
            let diverged = diverged.clone();
            // Entry resets the sync window — JS ran synchronously to reach this
            // host call. The post-await reset below restarts it for the chunk
            // that runs after the subagent resolves.
            let sync_budget = agent_sync_budget.clone();
            sync_budget.reset();
            let index = agent_index.get();
            agent_index.set(index + 1);
            // Convert opts on the sync side so the future holds only owned data.
            let opts_json = opts
                .0
                .and_then(|o| js_to_json(&ctx, o.into_value()).ok())
                .unwrap_or(serde_json::Value::Null);
            let ctx2 = ctx.clone();
            async move {
                // Lifetime agent cap: `index` is 0-based, so the (index+1)th
                // call exceeding the cap throws (matches CC's pre-increment gate).
                if index >= WORKFLOW_AGENT_CAP {
                    let msg = format!("Workflow agent cap ({WORKFLOW_AGENT_CAP}) exceeded.");
                    let thrown = msg.into_js(&ctx2)?;
                    return Err(ctx2.throw(thrown));
                }
                // Token-budget pre-call gate: reject before spawning once spent
                // has reached a positive total. In parallel/pipeline the rejected
                // thunk degrades that slot to null.
                if host.budget_exhausted() {
                    let msg = "Workflow token budget exhausted. Stopping further agent() calls."
                        .to_string();
                    let thrown = msg.into_js(&ctx2)?;
                    return Err(ctx2.throw(thrown));
                }
                let opts: WorkflowAgentOpts = serde_json::from_value(opts_json).unwrap_or_default();
                let label = opts.label.clone().unwrap_or_else(|| derive_label(&prompt));
                let phase_title = opts.phase.clone();
                // The resume cache key for this call (lookup + record-after-run).
                let cache_key = crate::host::AgentCacheKey::new(prompt.clone(), &opts);

                // Resume replay: while the cursor has not diverged, consult the
                // journal before doing any work. A hit replays the cached result
                // with NO spawn (emitted as `state: done`, `cached: true`); the
                // first miss flips `diverged` permanently so every later call
                // re-spawns (longest-unchanged-prefix). The Start event is only
                // emitted on the spawn path so a replay row reads as a single
                // cached Done.
                if !diverged.get() {
                    if let Some(value) = host.cached_agent_result(&cache_key).await {
                        sync_budget.reset();
                        let result_preview = preview_value(&value);
                        host.push_progress(agent_event(
                            index,
                            WorkflowAgentState::Done,
                            label,
                            phase_title,
                            /*cached*/ true,
                            DoneDetails {
                                result_preview,
                                ..DoneDetails::default()
                            },
                        ));
                        return json_to_js(&ctx2, &value);
                    }
                    // First miss: the cursor diverges and stays diverged.
                    diverged.set(true);
                }

                host.push_progress(agent_event(
                    index,
                    WorkflowAgentState::Start,
                    label.clone(),
                    phase_title.clone(),
                    /*cached*/ false,
                    DoneDetails::default(),
                ));
                let agent_outcome = host.run_agent(prompt, opts).await;
                // Resolving the await hands control back to JS — open a fresh
                // sync window for the chunk that follows.
                sync_budget.reset();
                match agent_outcome {
                    Ok(result) => {
                        if let Some(tokens) = result.tokens {
                            host.record_agent_tokens(tokens);
                        }
                        // Journal this result so a future resume can replay it
                        // without re-spawning. Null results are skipped host-side.
                        host.record_agent_result(&cache_key, &result.value).await;
                        let result_preview = preview_value(&result.value);
                        host.push_progress(agent_event(
                            index,
                            WorkflowAgentState::Done,
                            label,
                            phase_title,
                            /*cached*/ false,
                            DoneDetails {
                                model: result.model.clone(),
                                tokens: result.tokens,
                                tool_calls: result.tool_calls,
                                duration_ms: result.duration_ms,
                                result_preview,
                                error: None,
                            },
                        ));
                        json_to_js(&ctx2, &result.value)
                    }
                    Err(message) => {
                        host.push_progress(agent_event(
                            index,
                            WorkflowAgentState::Error,
                            label,
                            phase_title,
                            /*cached*/ false,
                            DoneDetails {
                                error: Some(message.clone()),
                                ..DoneDetails::default()
                            },
                        ));
                        let thrown = message.into_js(&ctx2)?;
                        Err(ctx2.throw(thrown))
                    }
                }
            }
        };
        globals.set(
            "agent",
            Function::new(ctx.clone(), Async(agent))?.with_name("agent")?,
        )?;
    }

    // workflow(nameOrRef, args?) — depth-aware. At depth 0 (top-level run) it is
    // host-backed: a saved/`{scriptPath}` child workflow runs inline via
    // `run_nested_workflow`, sharing this run's governance (same host ⇒ same
    // semaphore, budget, journal, abort, agent counter). At depth >= 1 (already
    // inside a child) it always throws — the one-level nesting guard.
    {
        if depth == 0 {
            let host = workflow_host;
            let sync_budget = workflow_sync_budget;
            // The CHILD engine the host re-enters is invoked at depth+1, so its
            // own `workflow()` rejects.
            let child_depth = depth + 1;
            let workflow = move |ctx: Ctx<'js>, name_or_ref: String, args: Opt<Value<'js>>| {
                let host = host.clone();
                let sync_budget = sync_budget.clone();
                // Reaching this host call means JS ran synchronously to get here.
                sync_budget.reset();
                // Convert args on the sync side so the future holds owned data.
                let args_json = args
                    .0
                    .and_then(|value| js_to_json(&ctx, value).ok())
                    .unwrap_or(serde_json::Value::Null);
                let ctx2 = ctx.clone();
                async move {
                    let outcome = host
                        .run_nested_workflow(name_or_ref, args_json, child_depth)
                        .await;
                    // Resolving the await hands control back to JS — fresh window.
                    sync_budget.reset();
                    match outcome {
                        Ok(value) => json_to_js(&ctx2, &value),
                        Err(message) => {
                            let thrown = message.into_js(&ctx2)?;
                            Err(ctx2.throw(thrown))
                        }
                    }
                }
            };
            globals.set(
                "workflow",
                Function::new(ctx.clone(), Async(workflow))?.with_name("workflow")?,
            )?;
        } else {
            // One-level guard: a child workflow's `workflow()` always rejects.
            // Async so the signature matches the host-backed (depth-0) form.
            let guard = move |ctx: Ctx<'js>| {
                let ctx2 = ctx.clone();
                async move {
                    let thrown = WORKFLOW_NESTING_LIMIT_ERROR.into_js(&ctx2)?;
                    Err::<Value<'js>, _>(ctx2.throw(thrown))
                }
            };
            globals.set(
                "workflow",
                Function::new(ctx.clone(), Async(guard))?.with_name("workflow")?,
            )?;
        }
    }

    Ok(())
}

fn agent_event(
    index: i32,
    state: WorkflowAgentState,
    label: String,
    phase_title: Option<String>,
    cached: bool,
    details: DoneDetails,
) -> WorkflowProgressEvent {
    WorkflowProgressEvent::WorkflowAgent {
        index,
        state,
        label,
        phase_title,
        phase_index: None,
        agent_id: None,
        model: details.model,
        started_at: None,
        queued_at: None,
        last_progress_at: None,
        tokens: details.tokens,
        tool_calls: details.tool_calls,
        duration_ms: details.duration_ms,
        cached,
        result_preview: details.result_preview,
        prompt_preview: None,
        error: details.error,
        skipped: false,
    }
}

#[derive(Default)]
struct DoneDetails {
    model: Option<String>,
    tokens: Option<i64>,
    tool_calls: Option<i32>,
    duration_ms: Option<i64>,
    result_preview: Option<String>,
    error: Option<String>,
}

fn preview_value(value: &serde_json::Value) -> Option<String> {
    let text = match value {
        serde_json::Value::Null => return None,
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut preview: String = text.chars().take(400).collect();
    if text.chars().count() > 400 {
        preview.push_str("...");
    }
    Some(preview)
}

/// A short progress label derived from the prompt's first line.
fn derive_label(prompt: &str) -> String {
    let first = prompt.lines().next().unwrap_or("agent").trim();
    let truncated: String = first.chars().take(48).collect();
    if truncated.is_empty() {
        "agent".to_string()
    } else {
        truncated
    }
}

#[cfg(test)]
#[path = "engine.test.rs"]
mod tests;
