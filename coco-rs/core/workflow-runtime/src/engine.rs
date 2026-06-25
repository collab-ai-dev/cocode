//! The workflow execution engine: a sandboxed QuickJS context running the
//! script body wrapped in an async IIFE, with the DSL globals
//! (`agent`/`parallel`/`pipeline`/`phase`/`log`/`workflow` + `args`/`budget`)
//! bridged to a [`WorkflowHost`]. Single-threaded by design (rquickjs `Ctx`/
//! `Value` are `!Send`) — drive it on a tokio current-thread runtime /
//! `LocalSet`. See `docs/coco-rs/workflow-runtime-plan.md`.

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
use crate::error::TimeoutSnafu;
use crate::error::WorkflowRuntimeError;
use crate::host::WorkflowAgentOpts;
use crate::host::WorkflowHost;

/// Default per-run wall-clock budget ().
pub const WORKFLOW_VM_TIMEOUT: Duration = Duration::from_secs(30);

/// JS-defined DSL combinators + `budget`, evaluated at context init. `parallel`
/// is a true barrier (`Promise.allSettled`); `pipeline` flows each item through
/// all stages independently..
const DSL_COMBINATORS: &str = r#"(() => {
  globalThis.parallel = async function parallel(funcs) {
    if (!Array.isArray(funcs)) throw new TypeError('parallel() expects an array of functions');
    for (const f of funcs)
      if (typeof f !== 'function')
        throw new TypeError('parallel() expects functions. Wrap each call: () => agent(...)');
    const settled = await Promise.allSettled(funcs.map(f => f()));
    return settled.map(s => s.status === 'fulfilled' ? s.value : null);
  };
  globalThis.pipeline = async function pipeline(items, ...stages) {
    if (!Array.isArray(items)) throw new TypeError('pipeline() expects an array as its first argument');
    const settled = await Promise.allSettled(items.map(async (item, i) => {
      let value = await item;
      for (const stage of stages) {
        if (value === null) break;
        value = await stage(value, item, i);
      }
      return value;
    }));
    return settled.map(s => s.status === 'fulfilled' ? s.value : null);
  };
  globalThis.workflow = async function workflow() {
    throw new Error('workflow() nesting is not available in this build yet.');
  };
})()"#;

/// The workflow execution engine.
pub struct WorkflowEngine;

impl WorkflowEngine {
    /// Run `script` to completion, returning its resolved JSON value. `args` is
    /// exposed to the script as the global `args`; `host` backs `agent()` and
    /// progress; `cancel` + `timeout` bound the run.
    /// MUST be awaited on a tokio current-thread runtime inside a `LocalSet`
    /// (the engine is `!Send`).
    pub async fn run(
        script: String,
        args: serde_json::Value,
        host: Arc<dyn WorkflowHost>,
        cancel: CancellationToken,
        timeout: Duration,
    ) -> Result<serde_json::Value, WorkflowRuntimeError> {
        let runtime = AsyncRuntime::new().map_err(setup_err)?;
        let context = AsyncContext::full(&runtime).await.map_err(setup_err)?;

        let deadline = Instant::now() + timeout;
        runtime.set_memory_limit(128 * 1024 * 1024).await;
        {
            let cancel = cancel.clone();
            runtime
                .set_interrupt_handler(Some(Box::new(move || {
                    Instant::now() >= deadline || cancel.is_cancelled()
                })))
                .await;
        }

        // Setup: sandbox + DSL combinators + host-backed globals. Sync block.
        let setup_host = host.clone();
        let setup_args = args.clone();
        context
            .with(move |ctx| -> rquickjs::Result<()> {
                crate::install_sandbox(&ctx)?;
                ctx.eval::<(), _>(DSL_COMBINATORS)?;
                install_globals(&ctx, setup_host, &setup_args)?;
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

        let outcome = tokio::select! {
            r = run_fut => r,
            _ = cancel.cancelled() => Err(CancelledSnafu.build()),
            _ = tokio::time::sleep_until(deadline.into()) => {
                cancel.cancel();
                Err(TimeoutSnafu { timeout }.build())
            }
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

/// Install the host-backed globals: `agent` (async), `phase`/`log` (sync),
/// `args` (frozen JSON), and `budget` (frozen).
fn install_globals<'js>(
    ctx: &Ctx<'js>,
    host: Arc<dyn WorkflowHost>,
    args: &serde_json::Value,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let agent_index = Rc::new(Cell::new(0i32));
    let phase_index = Rc::new(Cell::new(0i32));

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
        globals.set(
            "log",
            Func::from(move |message: String| {
                host.push_progress(WorkflowProgressEvent::WorkflowLog { message });
            }),
        )?;
    }

    // phase(title) — sync; assigns the next phase index.
    {
        let host = host.clone();
        globals.set(
            "phase",
            Func::from(move |title: String| {
                let index = phase_index.get();
                phase_index.set(index + 1);
                host.push_progress(WorkflowProgressEvent::WorkflowPhase { index, title });
            }),
        )?;
    }

    // agent(prompt, opts?) — async; spawns one subagent and awaits its result.
    {
        let agent = move |ctx: Ctx<'js>, prompt: String, opts: Opt<Object<'js>>| {
            let host = host.clone();
            let index = agent_index.get();
            agent_index.set(index + 1);
            // Convert opts on the sync side so the future holds only owned data.
            let opts_json = opts
                .0
                .and_then(|o| js_to_json(&ctx, o.into_value()).ok())
                .unwrap_or(serde_json::Value::Null);
            let ctx2 = ctx.clone();
            async move {
                let opts: WorkflowAgentOpts = serde_json::from_value(opts_json).unwrap_or_default();
                let label = opts.label.clone().unwrap_or_else(|| derive_label(&prompt));
                let phase_title = opts.phase.clone();
                host.push_progress(agent_event(
                    index,
                    WorkflowAgentState::Start,
                    label.clone(),
                    phase_title.clone(),
                    DoneDetails::default(),
                ));
                match host.run_agent(prompt, opts).await {
                    Ok(result) => {
                        if let Some(tokens) = result.tokens {
                            host.record_agent_tokens(tokens);
                        }
                        let result_preview = preview_value(&result.value);
                        host.push_progress(agent_event(
                            index,
                            WorkflowAgentState::Done,
                            label,
                            phase_title,
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

    Ok(())
}

fn agent_event(
    index: i32,
    state: WorkflowAgentState,
    label: String,
    phase_title: Option<String>,
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
        tokens: details.tokens,
        tool_calls: details.tool_calls,
        duration_ms: details.duration_ms,
        cached: false,
        result_preview: details.result_preview,
        prompt_preview: None,
        error: details.error,
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
