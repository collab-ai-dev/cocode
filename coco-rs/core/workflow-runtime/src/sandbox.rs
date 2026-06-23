//! The in-VM determinism shim and intrinsic hardening, ported verbatim from
//! claude-code's `runtime.ts` (`DETERMINISM_SHIM` obf `N0p`@416417,
//! `VM_HARDENING_PROGRAM` obf `KGe`@411340). These are JS programs evaluated in
//! the workflow context at init — the same technique the reference uses, engine
//! independent. They are defense-in-depth alongside the *static* AST check in
//! `coco_workflow::meta` (`isNonDeterministic`).

/// Runtime error thrown when a workflow touches `Date.now()` / `new Date()`.
pub const DATE_ERROR_MESSAGE: &str = "Date.now() / new Date() are unavailable in workflow scripts (breaks resume). Stamp results after the workflow returns, or pass timestamps via args.";

/// Runtime error thrown when a workflow touches `Math.random()`.
pub const RANDOM_ERROR_MESSAGE: &str = "Math.random() is unavailable in workflow scripts (breaks resume). For N independent samples, include the index in the agent label or prompt.";

const DETERMINISM_SHIM_TEMPLATE: &str = r#"(() => {
  const NOW_ERR = __NOW_ERR__;
  const RANDOM_ERR = __RANDOM_ERR__;
  Math.random = function random() { throw new Error(RANDOM_ERR) };
  const RealDate = Date;
  RealDate.now = function now() { throw new Error(NOW_ERR) };
  function ShimDate(...a) {
    if (!new.target) throw new Error(NOW_ERR);          // bare Date()
    if (a.length === 0) throw new Error(NOW_ERR);       // new Date() with no args
    return Reflect.construct(RealDate, a, new.target);  // new Date(2020, 0, 1) is fine
  }
  ShimDate.now = RealDate.now;
  ShimDate.parse = RealDate.parse;
  ShimDate.UTC = RealDate.UTC;
  ShimDate.prototype = RealDate.prototype;
  RealDate.prototype.constructor = ShimDate;
  Object.freeze(RealDate);
  globalThis.Date = ShimDate;
})()"#;

/// The in-VM determinism shim, with the error messages injected. Makes
/// `Math.random()`, `Date.now()`, bare `Date()` and argless `new Date()` throw,
/// while still allowing `new Date(2020, 0, 1)` (explicit args are deterministic).
pub fn determinism_shim() -> String {
    DETERMINISM_SHIM_TEMPLATE
        .replace("__NOW_ERR__", &js_string_literal(DATE_ERROR_MESSAGE))
        .replace("__RANDOM_ERR__", &js_string_literal(RANDOM_ERROR_MESSAGE))
}

/// Intrinsic hardening: freeze `Error.prepareStackTrace` to a no-op and delete
/// dangerous globals (those that exist in QuickJS — deleting an absent global is
/// a harmless no-op). `eval` is intentionally left intact; dynamic codegen is
/// not a meaningful escape vector here because the determinism shim and the
/// fixed host-function surface bound what a script can do.
pub const HARDENING_PROGRAM: &str = r#"(() => {
  try {
    Object.defineProperty(Error, 'prepareStackTrace', {
      value: (err) => String((err && err.stack) || err),
      writable: false, configurable: false,
    });
  } catch (_) {}
  for (const g of ['ShadowRealm','WebAssembly','FinalizationRegistry','WeakRef',
                   'Atomics','SharedArrayBuffer','queueMicrotask']) {
    try { delete globalThis[g]; } catch (_) {}
  }
})()"#;

fn js_string_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
#[path = "sandbox.test.rs"]
mod tests;
