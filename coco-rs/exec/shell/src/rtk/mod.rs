//! RTK (Rust Token Killer) Bash-output compression — phase 1 (subprocess tier).
//!
//! When a healthy `rtk` (or `rr-rtk`) binary is on `$PATH`, [`RtkRewriter`]
//! rewrites a Bash command string (`git status` → `rtk git status`) via the
//! stable `rtk rewrite "<cmd>"` CLI contract, immediately before spawn and
//! **after** permission evaluation, read-only classification and the sandbox
//! decision have all run on the *original* command (design §4.2).
//!
//! The public API ([`RtkRewriter::rewrite`]) is **infallible**: every failure
//! maps to a [`RewriteOutcome::Passthrough`] carrying a [`PassthroughReason`],
//! so a broken / missing / slow rtk can never fail a Bash call — it only
//! declines to compress. No error type crosses the crate boundary.
//!
//! Design: `docs/coco-rs/rtk-integration-design.md`.

mod detect;
mod filter;
mod rewrite;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

use std::time::Instant;

use coco_config::RtkConfig;
use coco_config::RtkMode;
use tokio::sync::OnceCell;

pub use detect::RtkBinary;
pub use detect::RtkFlavor;
pub(crate) use detect::RtkProbe;
pub use detect::RtkVersion;

/// Which integration tier produced a compressed Bash result. Recorded in the
/// Bash result envelope (`rtk` field) and metrics via [`RtkTier::as_str`] —
/// the single source of the wire strings. Phase 1 only ever emits
/// [`RtkTier::External`]; [`RtkTier::Builtin`] is reserved for the phase-2
/// embedded post-exec filter core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtkTier {
    Builtin,
    External,
}

impl RtkTier {
    pub fn as_str(self) -> &'static str {
        match self {
            RtkTier::Builtin => "builtin",
            RtkTier::External => "external",
        }
    }
}

/// Result of a rewrite attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteOutcome {
    /// `rtk rewrite` returned exit 0|3 with a non-empty, fully-accounted-for
    /// rewrite. Execute this string instead of the original.
    Rewritten(String),
    /// Execute the original command unchanged. The reason is recorded for
    /// metrics / tracing.
    Passthrough(PassthroughReason),
}

/// Why a command was not rewritten. Every variant terminates in "run the
/// original command" — none is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassthroughReason {
    /// No `rtk` / `rr-rtk` binary detected on `$PATH` (or the configured path).
    BinaryMissing,
    /// Detected binary is older than the minimum supported rewrite contract.
    VersionTooOld,
    /// `run_in_background=true`: rtk buffers-then-prints, which would stall
    /// incremental `TaskOutput` streaming.
    Background,
    /// The sandbox will wrap this command: rtk's SQLite history write under
    /// `~/.local/share/rtk` is blocked / EROFS under ReadOnly/Strict.
    Sandboxed,
    /// First command word is in the coco-side `rtk.exclude_commands` list.
    Excluded,
    /// `rtk rewrite` exit 1 — no rtk equivalent for this command.
    NoEquivalent,
    /// `rtk rewrite` exit 2 — a *host* deny rule matched (informational only;
    /// coco ran its own permission engine before this).
    HostDeny,
    /// The rewrite probe exceeded `rtk.rewrite_timeout_ms` and was killed.
    Timeout,
    /// The rtk process could not be spawned.
    SpawnError,
    /// rr-rtk fixup: a rewritten segment didn't start with the `rtk` token, so
    /// the rewrite could not be fully accounted for (§4.5).
    ShapeMismatch,
}

impl PassthroughReason {
    /// Stable metric tag value (`coco.rtk.engine_total{reason=...}`).
    pub fn as_metric_str(self) -> &'static str {
        match self {
            PassthroughReason::BinaryMissing => "binary_missing",
            PassthroughReason::VersionTooOld => "version_too_old",
            PassthroughReason::Background => "background",
            PassthroughReason::Sandboxed => "sandboxed",
            PassthroughReason::Excluded => "excluded",
            PassthroughReason::NoEquivalent => "no_equivalent",
            PassthroughReason::HostDeny => "host_deny",
            PassthroughReason::Timeout => "timeout",
            PassthroughReason::SpawnError => "spawn_error",
            PassthroughReason::ShapeMismatch => "shape_mismatch",
        }
    }
}

/// Execution-site facts the skip conditions (§4.3) need. Computed by the Bash
/// tool from the *original* command before the rewrite runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteSite {
    /// `BashInput.run_in_background`.
    pub background: bool,
    /// The sandbox snapshot decided it will wrap this command.
    pub sandboxed: bool,
}

/// The Bash output-compression seam. A backend acts at one or both of two
/// lifecycle points (design §3.5): a **pre-spawn rewrite** (external tier —
/// `git status` → `rtk git status`, whose tool output the engine then sees
/// compressed) and a **post-exec filter** (builtin tier — compress the captured
/// stdout of the unmodified command in-process). [`RtkRewriter`] is the only
/// implementation today and does both, arbitrated by [`RtkMode`]; the trait
/// exists so `BashTool` depends on the seam and its two capability predicates
/// rather than on a concrete backend or on `RtkMode`. The whole API is
/// **infallible**: a rewrite maps to [`RewriteOutcome::Passthrough`] and a
/// filter to `None`, so a broken backend only declines to compress.
#[async_trait::async_trait]
pub trait BashOutputRewriter: std::fmt::Debug + Send + Sync {
    /// Rewrite `command` for output compression, or decide to pass it through.
    async fn rewrite(&self, command: &str, site: RewriteSite) -> RewriteOutcome;

    /// Whether this backend performs a pre-spawn rewrite. When `false`,
    /// `BashTool` skips [`rewrite`](BashOutputRewriter::rewrite) and spawns the
    /// original command. **Required, no default:** a silent `true` would opt a
    /// post-exec-only backend into modifying the spawned command it never meant
    /// to touch — each backend must declare its tiers explicitly.
    fn does_pre_spawn_rewrite(&self) -> bool;

    /// Whether this backend performs post-exec filtering. When `true`,
    /// `BashTool` calls [`filter_output`](BashOutputRewriter::filter_output) on
    /// the captured stdout — but never when a pre-spawn rewrite already fired
    /// for the same call (no double filtering, §3.5). Required, no default.
    fn does_post_exec_filter(&self) -> bool;

    /// Post-exec output compression. Given the original command, its exit code,
    /// and captured stdout, return compressed text or `None` to keep the raw
    /// output. Infallible — a filter panic degrades to `None`. Defaults to `None`
    /// for pre-spawn-only backends; it is only ever called when
    /// [`does_post_exec_filter`](BashOutputRewriter::does_post_exec_filter) is `true`.
    async fn filter_output(
        &self,
        _command: &str,
        _exit_code: i32,
        _stdout: &str,
    ) -> Option<String> {
        None
    }
}

/// Session-wide RTK rewriter, shared via `Arc<dyn BashOutputRewriter>` on
/// `ToolUseContext` (mirrors `shell_provider`). The binary is probed **once per
/// session** through an [`OnceCell`]; the config is captured at construction.
#[derive(Debug)]
pub struct RtkRewriter {
    config: RtkConfig,
    probe: OnceCell<RtkProbe>,
}

impl RtkRewriter {
    pub fn new(config: RtkConfig) -> Self {
        Self {
            config,
            probe: OnceCell::new(),
        }
    }

    async fn rewrite_inner(&self, command: &str, site: RewriteSite) -> RewriteOutcome {
        // Cheap, spawn-free vetoes first.
        if site.background {
            return RewriteOutcome::Passthrough(PassthroughReason::Background);
        }
        if site.sandboxed {
            return RewriteOutcome::Passthrough(PassthroughReason::Sandboxed);
        }
        if self.is_excluded(command) {
            return RewriteOutcome::Passthrough(PassthroughReason::Excluded);
        }

        // Probe once per session.
        let binary = match self.probe().await {
            RtkProbe::Found(binary) => binary,
            RtkProbe::Missing => {
                return RewriteOutcome::Passthrough(PassthroughReason::BinaryMissing);
            }
            RtkProbe::VersionTooOld => {
                return RewriteOutcome::Passthrough(PassthroughReason::VersionTooOld);
            }
        };

        rewrite::run_rewrite(binary, command, self.config.rewrite_timeout_ms).await
    }

    async fn probe(&self) -> &RtkProbe {
        self.probe.get_or_init(|| detect::probe(&self.config)).await
    }

    /// coco-side skip list, matched on the first command word (safe env-var
    /// prefixes stripped) before the probe spawns.
    fn is_excluded(&self, command: &str) -> bool {
        if self.config.exclude_commands.is_empty() {
            return false;
        }
        let first = crate::get_first_word_prefix(command)
            .or_else(|| command.split_whitespace().next().map(str::to_string));
        first.is_some_and(|word| self.config.exclude_commands.contains(&word))
    }
}

#[async_trait::async_trait]
impl BashOutputRewriter for RtkRewriter {
    /// Runs the cheap skip conditions first (no subprocess), probes the binary
    /// once per session, then invokes `rtk rewrite`. Emits one decision metric +
    /// `debug!` trace per call.
    async fn rewrite(&self, command: &str, site: RewriteSite) -> RewriteOutcome {
        let start = Instant::now();
        let outcome = self.rewrite_inner(command, site).await;
        let latency_ms = start.elapsed().as_millis() as i64;
        emit_decision(command, &outcome, latency_ms);
        outcome
    }

    /// External tiers only ([`RtkMode::ExternalFirst`] / [`RtkMode::ExternalOnly`]).
    /// Under the default `BuiltinFirst` the command is spawned unmodified and
    /// compressed post-exec instead.
    fn does_pre_spawn_rewrite(&self) -> bool {
        matches!(
            self.config.mode,
            RtkMode::ExternalFirst | RtkMode::ExternalOnly
        )
    }

    /// Every tier except [`RtkMode::ExternalOnly`]. Combined with `BashTool`'s
    /// no-double-filtering guard, this yields the §3.5 arbitration: `BuiltinFirst`
    /// / `BuiltinOnly` always post-filter; `ExternalFirst` post-filters only when
    /// the pre-spawn rewrite did not fire.
    fn does_post_exec_filter(&self) -> bool {
        !matches!(self.config.mode, RtkMode::ExternalOnly)
    }

    async fn filter_output(&self, command: &str, exit_code: i32, stdout: &str) -> Option<String> {
        filter::apply_rtk_filter(command, exit_code, stdout).await
    }
}

/// Best-effort observability. `command_prefix` is the resolved command word via
/// [`crate::get_first_word_prefix`] (safe env-var prefixes stripped, and an
/// unsafe inline assignment like `GITHUB_TOKEN=…` redacted to `<other>` rather
/// than logged) so sensitive arguments never reach logs / metrics. `duration_ms`
/// uses the `common/otel` standard field name.
fn emit_decision(command: &str, outcome: &RewriteOutcome, duration_ms: i64) {
    let command_prefix = crate::get_first_word_prefix(command);
    let command_prefix = command_prefix.as_deref().unwrap_or("<other>");
    let (tier, reason) = match outcome {
        RewriteOutcome::Rewritten(_) => ("external", "rewritten"),
        RewriteOutcome::Passthrough(reason) => ("skip", reason.as_metric_str()),
    };
    tracing::debug!(
        command_prefix,
        outcome = reason,
        duration_ms,
        "rtk rewrite decision"
    );
    // Capability-level metrics tagged by engine, so a second `BashOutputRewriter`
    // backend shares the namespace (`engine="rtk"` here).
    coco_otel::metrics::record_counter(
        "coco.output_rewrite.decision_total",
        1,
        &[("engine", "rtk"), ("tier", tier), ("reason", reason)],
    );
    coco_otel::metrics::record_histogram(
        "coco.output_rewrite.duration_ms",
        duration_ms,
        &[("engine", "rtk")],
    );
}
