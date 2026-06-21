//! Layer 2 + Layer 4 of the tool-filter pipeline.
//!
//! See `docs/coco-rs/feature-gates-and-tool-filtering.md` §7.
//!
//! - [`ToolOverrides`] — Layer 2: per-model adjustments to the universal
//!   tool baseline. Extra tools the model adds (e.g. gpt-5's
//!   `apply_patch`), baseline tools the model excludes (e.g. gpt-5's
//!   `Edit`). Configurable per-model in settings.json.
//! - [`ToolFilter`] — Layer 4: subagent allow/deny lists derived from
//!   `AgentDefinition.allowed_tools` / `disallowed_tools`.
//!
//! Both default to "no adjustment" so callers that haven't constructed a
//! specific instance get the permissive identity element. Subagents can
//! only **narrow** the set, never widen it.
//!
//! Identities are typed as [`ToolId`] (built-in / MCP / custom) rather
//! than raw strings — typo-safe for built-ins, structured for MCP, and
//! still open for plugin / model-specific tools via `Custom`.

use std::collections::HashSet;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

use crate::ToolId;
use crate::ToolName;

/// Per-model tool overrides against the universal baseline.
///
/// The baseline is "every registered tool that isn't model-specific".
/// A model declares only its delta — what it adds beyond the baseline
/// and what it excludes from the baseline — so introducing a new
/// built-in tool doesn't require updating every model's entry.
///
/// Examples:
/// - **Default** model (Claude family): [`ToolOverrides::none()`].
/// - **gpt-5**: `extra = {apply_patch}`, `excluded = {Edit}`.
/// - A hypothetical voice-only role: `excluded = {Bash, Edit, Write}`.
///
/// Built once at session start (or on `/model` switch) and stored on
/// `ToolUseContext` as `Arc` so the per-turn ctx clone is cheap.
///
/// Serialized as `{ "extra": [...], "excluded": [...] }` so users can
/// declare per-model overrides directly in settings.json under
/// `providers.<name>.models.<id>.tool_overrides`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOverrides {
    /// Tools this model accepts on top of the baseline.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    extra: HashSet<ToolId>,
    /// Baseline tools the model rejects.
    /// `excluded` takes precedence over `extra` if an id appears in both.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    excluded: HashSet<ToolId>,
}

impl ToolOverrides {
    /// Identity element — empty diff, the model uses the full baseline as-is.
    pub fn none() -> Self {
        Self::default()
    }

    /// Builder: declare a tool the model adds beyond the baseline.
    pub fn with_extra(mut self, id: impl Into<ToolId>) -> Self {
        self.extra.insert(id.into());
        self
    }

    /// Builder: declare a baseline tool the model rejects.
    pub fn with_excluded(mut self, id: impl Into<ToolId>) -> Self {
        self.excluded.insert(id.into());
        self
    }

    /// Layer `other` on top of `self`. `other`'s entries win where they
    /// overlap (`excluded` always beats `extra`). Used at config-resolve
    /// time to compose the built-in registry diff with the user's
    /// settings.json `tool_overrides`.
    pub fn merge(mut self, other: &ToolOverrides) -> Self {
        for id in &other.extra {
            self.extra.insert(id.clone());
        }
        for id in &other.excluded {
            self.excluded.insert(id.clone());
            // If the user explicitly excluded a tool, drop any prior
            // "extra" entry for the same id so `excluded` wins cleanly.
            self.extra.remove(id);
        }
        self
    }

    /// Whether the active model accepts the given tool id.
    ///
    /// `excluded` is checked first, so an id in both `extra` and
    /// `excluded` is treated as excluded.
    pub fn permits(&self, id: &ToolId) -> bool {
        !self.excluded.contains(id)
    }

    /// Whether the model added this tool beyond the baseline.
    pub fn is_extra(&self, id: &ToolId) -> bool {
        self.extra.contains(id) && !self.excluded.contains(id)
    }

    /// String-based convenience for callers that already hold a wire-
    /// format name (typically the registry, which iterates by name).
    /// Falls back to `Custom` for unknown strings via `ToolId::from_str`.
    pub fn permits_name(&self, name: &str) -> bool {
        let id = ToolId::from_str(name).expect("ToolId::from_str is infallible");
        self.permits(&id)
    }

    /// The builtin this model uses to create / fully write a file, derived
    /// from the override diff (no per-model table). Claude family → `Write`;
    /// gpt-5 family (excludes `Write`, adds `apply_patch`) → `ApplyPatch`.
    /// Shares [`ToolName::file_mutation_tool`] with the name-list resolver so
    /// override-holding callers (e.g. compaction) and tool-list-holding callers
    /// (prompts) agree. See [`ToolName::write_tool_for`].
    pub fn write_tool(&self) -> ToolName {
        ToolName::file_mutation_tool(
            ToolName::Write,
            self.permits(&ToolId::Builtin(ToolName::Write)),
            self.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)),
        )
    }

    /// Edit-operation counterpart of [`Self::write_tool`].
    pub fn edit_tool(&self) -> ToolName {
        ToolName::file_mutation_tool(
            ToolName::Edit,
            self.permits(&ToolId::Builtin(ToolName::Edit)),
            self.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)),
        )
    }
}

/// Subagent / role-level allow + deny list (Layer 4).
///
/// `AgentDefinition.allowed_tools` (whitelist) is interpreted as
/// "use only these"; `disallowed_tools` is the deny-list applied on top.
/// A subagent that left both empty inherits the parent's filter via
/// `unrestricted()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolFilter {
    /// `None` = no whitelist; every id is allowed unless denied.
    /// `Some(set)` = restricted to this set.
    allowed: Option<HashSet<ToolId>>,
    /// Always denies these ids regardless of the allow list.
    disallowed: HashSet<ToolId>,
}

impl ToolFilter {
    /// Permissive default — the filter is the identity (allows every tool).
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Build from explicit allow + deny pieces. Empty `allowed` ⇒ no
    /// whitelist (treated as `None`).
    ///
    /// Inputs are wire-format strings (settings.json / agent definition);
    /// each is parsed via `ToolId::from_str` (infallible — unknown names
    /// land in `Custom`).
    pub fn new(allowed: Vec<String>, disallowed: Vec<String>) -> Self {
        let parse = |s: String| ToolId::from_str(&s).expect("ToolId::from_str is infallible");
        let allowed = if allowed.is_empty() {
            None
        } else {
            Some(allowed.into_iter().map(parse).collect())
        };
        Self {
            allowed,
            disallowed: disallowed.into_iter().map(parse).collect(),
        }
    }

    /// Whether the given tool id passes this filter.
    pub fn allows(&self, id: &ToolId) -> bool {
        if self.disallowed.contains(id) {
            return false;
        }
        match &self.allowed {
            Some(set) => set.contains(id),
            None => true,
        }
    }

    /// String-based convenience for the registry filter pipeline.
    pub fn allows_name(&self, name: &str) -> bool {
        let id = ToolId::from_str(name).expect("ToolId::from_str is infallible");
        self.allows(&id)
    }

    /// Tighten `self` with `parent`'s constraints — used at subagent
    /// spawn time so a child's `AgentDefinition.allowed_tools` cannot
    /// widen the parent's filter.
    ///
    /// Composition rules:
    /// - `allowed`: intersection (any whitelist on either side narrows).
    ///   `Some ∩ Some` is the set intersection;  `Some ∩ None`
    ///   keeps the `Some`; `None ∩ None` stays `None`.
    /// - `disallowed`: union (every deny entry from either side carries
    ///   over).
    ///
    /// TS parity: subagent spawn flows through `runAgent.ts` which
    /// inherits the parent's tool filter and then narrows it with the
    /// agent definition. The Rust analog has historically only used the
    /// child's filter; this method closes the widening gap.
    pub fn narrow_with(self, parent: &ToolFilter) -> Self {
        let allowed = match (self.allowed, &parent.allowed) {
            (Some(child), Some(parent_set)) => {
                Some(child.intersection(parent_set).cloned().collect())
            }
            (Some(child), None) => Some(child),
            (None, Some(parent_set)) => Some(parent_set.clone()),
            (None, None) => None,
        };
        let mut disallowed = self.disallowed;
        for id in &parent.disallowed {
            disallowed.insert(id.clone());
        }
        Self {
            allowed,
            disallowed,
        }
    }
}

#[cfg(test)]
#[path = "tool_filter.test.rs"]
mod tests;
