//! Phase-2 embedded post-exec filter stage (design §3.3).
//!
//! After a Bash command runs, [`apply_rtk_filter`] matches the captured stdout
//! against rtk's declarative TOML filter registry (63 builtin filters plus the
//! user / project `.rtk/filters.toml` files rtk itself reads) and returns a
//! compressed replacement — or `None` to keep the raw output. The engine is
//! text→text and runs entirely in-process: no rtk binary, so unlike the phase-1
//! external rewrite tier it also runs for **sandboxed** commands (no subprocess,
//! no SQLite write). It fires only on a *completed foreground* command (both
//! `execute_foreground` and the TaskRuntime `Terminal` arm); a command launched
//! with `run_in_background: true` streams its output elsewhere and is not
//! filtered — incremental post-exec filtering of a live stream is out of scope.
//!
//! **Only single commands are filtered.** The builtin filters match the *first*
//! word of the command, but a compound / piped command's captured stdout is the
//! *combined* output of every segment; applying a first-word filter there would
//! truncate a trailing command's data. Compound / piped commands pass through raw.
//!
//! **Bounded downside — every path degrades to raw output:**
//! - no filter matches / compound command / stderr-only filter → `None`;
//! - the filtered text is not smaller than raw → `None`;
//! - the filter panics → caught at the [`tokio::task::spawn_blocking`] join
//!   boundary (coco builds `panic = "unwind"`, so the unwind is contained) →
//!   `None`. Note the amplification risk: rtk's registry is a `lazy_static`
//!   singleton, so a panic *during first-use init* would poison it process-wide
//!   and every later call would re-panic (each still contained → raw, but paying
//!   a thread-spawn + unwind per Bash command). rtk's `load()` is panic-free
//!   today, so this is latent, not live.
//!
//! **Scope in v0.** Only the declarative TOML long-tail (terraform, helm, make,
//! jq, …) is available. The marquee git / cargo / pytest *family formatters*
//! live in the fork's `cmds` module, which the lib target does not yet expose
//! (it is coupled to the binary-only `Commands` enum); those commands fall
//! through to raw output until the upstream decouple lands. The custom-filter
//! trust consent *write* flow is likewise deferred: the registry loader already
//! trust-gates project filter files internally — silently skipping untrusted
//! ones, matching rtk's headless posture — which is the safe default.
//!
//! **Known skew.** rtk's TOML registry is a process-lifetime singleton that
//! reads the project `.rtk/filters.toml` relative to the process CWD on first
//! use. coco never changes its process CWD, so this resolves to the launch
//! directory; a Bash command whose effective cwd differs (worktree / multi-root
//! session) will not pick up *that* directory's project filter file. The 63
//! builtin filters and the user-global file are CWD-independent and always apply.

#[cfg(test)]
#[path = "filter.test.rs"]
mod tests;

use rtk::core::toml_filter::CompiledFilter;
use tracing::debug;

/// Outcome of applying a matched filter, decided on the blocking thread so
/// [`apply_rtk_filter`] owns the single metric-emission site (and so the apply
/// step is a pure function the tests can assert on directly).
#[derive(Debug, PartialEq, Eq)]
enum ApplyOutcome {
    /// The filter produced a genuinely smaller payload.
    Filtered(String),
    /// The filter's output was no smaller than raw — keep raw.
    NeverWorse,
}

/// Post-exec output compression via rtk's embedded TOML filter core. Returns
/// `Some(compressed)` only when a filter matched *and* the size guard confirms a
/// genuine reduction; `None` means "keep the raw output" for every other case
/// (opt-out, compound command, no match, stderr-only filter, guard rejection,
/// filter panic, empty output). Infallible — a filter can only decline to
/// compress, never fail the Bash call.
///
/// `exit_code` is accepted for the stable post-exec seam (the deferred family
/// formatters key failure-focus off it) but is unused by the TOML engine.
pub async fn apply_rtk_filter(command: &str, exit_code: i32, stdout: &str) -> Option<String> {
    let _ = exit_code; // TOML filters are command+text only; families (deferred) use it.

    if stdout.is_empty() {
        return None;
    }
    // Per-command opt-out: `RTK_DISABLED=1 <cmd>` — the same escape hatch the
    // external tier honors. `strip_disabled_prefix` peels env-assignment
    // prefixes; `rest` is the bare command used for matching (so a benign
    // `FOO=bar terraform plan` still matches `^terraform`).
    let (prefix, rest) = rtk::discover::registry::strip_disabled_prefix(command);
    if rtk::discover::registry::prefix_contains_rtk_disabled(prefix)
        || rtk::core::toml_filter::toml_disabled()
    {
        return None;
    }

    // All of the following is command-string-only work (microseconds) — do it
    // BEFORE cloning the (up-to-2 MB) stdout so the common no-match path pays no
    // heap copy or thread hop.
    let Some(filter) = matchable_filter(command, rest) else {
        emit_decision("miss");
        return None;
    };

    // Matched → the byte-heavy `apply_filter` regex work is now justified; run it
    // on a blocking thread so large-output filtering never stalls the async
    // runtime and a filter panic surfaces as a `JoinError` (degrade to raw)
    // instead of unwinding through coco.
    let stdout = stdout.to_string();
    let outcome = match tokio::task::spawn_blocking(move || apply_and_guard(filter, &stdout)).await
    {
        Ok(outcome) => outcome,
        Err(join_err) => {
            debug!(error = %join_err, "rtk post-exec filter panicked; using raw output");
            emit_decision("panic");
            return None;
        }
    };

    match outcome {
        ApplyOutcome::Filtered(out) => {
            emit_decision("filtered");
            Some(out)
        }
        ApplyOutcome::NeverWorse => {
            emit_decision("never_worse");
            None
        }
    }
}

/// Cheap, command-string-only: decide whether a TOML filter applies and return
/// it. `None` (⇒ no compression) for:
/// - a **compound / piped** command — the builtin filters match the *first*
///   word (`^make\b`, `^jq\b`, …) but the captured stdout is the *combined*
///   output of every segment, so applying a first-word filter would truncate a
///   trailing command's data (unrecoverably). Filter single commands only.
/// - no registry match.
/// - a filter that folds in **stderr** (`filter_stderr`) — this stdout-only seam
///   cannot merge stderr the way rtk-native does, and half-applying it could emit
///   the filter's `on_empty` text over a failed run's real error. Skip it.
fn matchable_filter(command: &str, stripped: &str) -> Option<&'static CompiledFilter> {
    if crate::split_compound_command(command).len() > 1 {
        return None;
    }
    let filter = rtk::core::toml_filter::find_matching_filter(stripped)?;
    if filter.filter_stderr {
        return None;
    }
    Some(filter)
}

/// Apply a matched filter and enforce the never-worse guarantee ourselves rather
/// than depend on rtk's `never_worse` return-by-reference identity (which a
/// future rtk refactor could silently break). rtk estimates tokens as `bytes/4`,
/// so a byte-length comparison is a faithful, conservative proxy: keep the
/// filtered output only when it is no larger than raw.
fn apply_and_guard(filter: &CompiledFilter, stdout: &str) -> ApplyOutcome {
    let filtered = rtk::core::toml_filter::apply_filter(filter, stdout);
    if filtered.len() > stdout.len() {
        return ApplyOutcome::NeverWorse;
    }
    // Defensive ANSI strip: a matched filter that does not set `strip_ansi` can
    // pass escapes through, and model-facing tool results must stay ANSI-free.
    ApplyOutcome::Filtered(rtk::core::utils::strip_ansi(&filtered))
}

/// Builtin-tier decision metric, sharing the capability namespace + `engine`
/// tag with the phase-1 emitter so a future backend slots in unchanged.
fn emit_decision(reason: &'static str) {
    coco_otel::metrics::record_counter(
        "coco.output_rewrite.decision_total",
        1,
        &[("engine", "rtk"), ("tier", "builtin"), ("reason", reason)],
    );
}
