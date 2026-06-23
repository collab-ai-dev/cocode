//! JavaScript runtime for Dynamic Workflows.
//!
//! Mirrors claude-code's `node:vm`-based execution model using an embedded
//! QuickJS engine (`rquickjs`): a sandboxed context per run, the DSL globals
//! (`agent`/`parallel`/`pipeline`/`phase`/`log`/`workflow` + `args`/`budget`),
//! a runtime determinism shim, and intrinsic hardening. See
//! `docs/coco-rs/workflow-runtime-plan.md`.
//!
//! The engine is single-threaded by design (rquickjs `Ctx`/`Value` are
//! `!Send`): drive [`WorkflowEngine::run`] on a tokio current-thread runtime
//! inside a `LocalSet`. The concrete [`WorkflowHost`] impl (backed by the real
//! subagent system) lives at a layer that has those handles.

mod convert;
mod engine;
mod error;
mod host;
mod sandbox;

pub use convert::js_to_json;
pub use convert::json_to_js;
pub use engine::WORKFLOW_VM_TIMEOUT;
pub use engine::WorkflowEngine;
pub use error::WorkflowRuntimeError;
pub use host::WorkflowAgentOpts;
pub use host::WorkflowAgentResult;
pub use host::WorkflowHost;
pub use sandbox::DATE_ERROR_MESSAGE;
pub use sandbox::RANDOM_ERROR_MESSAGE;

use rquickjs::Context;
use rquickjs::Ctx;
use rquickjs::Runtime;

/// Smoke check that the embedded QuickJS engine builds and evaluates JS in this
/// environment.
pub fn eval_smoke() -> Result<i32, rquickjs::Error> {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| ctx.eval::<i32, _>("1 + 1"))
}

/// Install the runtime determinism shim + intrinsic hardening into a context,
/// matching claude-code's per-context init (hardening first, then the
/// determinism shim).
pub fn install_sandbox(ctx: &Ctx<'_>) -> Result<(), rquickjs::Error> {
    ctx.eval::<(), _>(sandbox::HARDENING_PROGRAM)?;
    ctx.eval::<(), _>(sandbox::determinism_shim().as_str())?;
    Ok(())
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
