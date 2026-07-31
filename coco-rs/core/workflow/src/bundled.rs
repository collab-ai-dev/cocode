//! Workflows compiled into the binary.
//!
//! A bundled workflow is an ordinary script string — the same thing a user
//! drops in `<config-dir>/workflows` — that happens to be embedded with
//! `include_str!`. The registry therefore stores **only the source**, and the
//! metadata is parsed back out of it on first use, so the script literal stays
//! the single source of truth for name / description / `whenToUse` / phases.
//! (The reference implementation keeps a duplicate metadata record and
//! interpolates it back into the script at registration; parsing once removes
//! the chance of the two drifting, and removes the unescaped-interpolation
//! hazard that comes with it.)

use std::sync::LazyLock;

use crate::WorkflowMeta;
use crate::parse_workflow_script;

/// Every bundled workflow's source, embedded at compile time.
const BUNDLED_SCRIPTS: &[&str] = &[include_str!("bundled/deep-research.js")];

/// A workflow shipped inside the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledWorkflow {
    /// Parsed from [`Self::script`]'s `export const meta` header.
    pub meta: WorkflowMeta,
    pub script: &'static str,
}

/// The bundled workflow registry, parsed once per process.
///
/// A script whose meta fails to parse is **dropped** rather than panicking — a
/// malformed bundled script must not take down a session. `bundled.test.rs`
/// asserts every embedded script parses (and is deterministic), which is what
/// turns that soft failure into a build-time one.
pub fn bundled_workflows() -> &'static [BundledWorkflow] {
    static REGISTRY: LazyLock<Vec<BundledWorkflow>> = LazyLock::new(|| {
        BUNDLED_SCRIPTS
            .iter()
            .filter_map(|script| {
                // Determinism is not checked here, matching the on-disk registry
                // scan: indexing a workflow is independent of whether it would
                // pass the launch-time check.
                parse_workflow_script(script, /*check_determinism*/ false)
                    .ok()
                    .map(|parsed| BundledWorkflow {
                        meta: parsed.meta,
                        script,
                    })
            })
            .collect()
    });
    &REGISTRY
}

/// Look up a bundled workflow by its `meta.name`.
pub fn bundled_workflow(name: &str) -> Option<&'static BundledWorkflow> {
    bundled_workflows()
        .iter()
        .find(|workflow| workflow.meta.name == name)
}

#[cfg(test)]
#[path = "bundled.test.rs"]
mod tests;
