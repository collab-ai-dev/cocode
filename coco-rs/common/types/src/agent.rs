use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use crate::ModelRole;
use crate::ReasoningEffort;

/// Built-in subagent types.
///
/// **Case is part of the contract.** TS treats `Explore` / `Plan` as PascalCase
/// (consumed by the case-sensitive one-shot set in `constants.ts`, by user
/// permission rules like `Agent(Explore)`, and by the
/// `tengu_agent_tool_selected` telemetry attribute). The remaining built-ins
/// (`general-purpose`, `statusline-setup`, `verification`, `coco-guide`)
/// stay lowercase. See `subagent-refactor-plan.md` § "Naming decision".
///
/// **Coco-rs rename**: TS `claude-code-guide` is renamed to `coco-guide`
/// to match the project identity. Per project policy (no backward-compat
/// shims) the TS string is not accepted as an alias — only `coco-guide`
/// (and its `coco_guide` underscore form) parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubagentType {
    GeneralPurpose,
    StatusLine,
    Explore,
    Plan,
    Verification,
    CocoGuide,
}

impl SubagentType {
    /// Canonical TS string form. Output side must always use this exact case.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => "general-purpose",
            Self::StatusLine => "statusline-setup",
            Self::Explore => "Explore",
            Self::Plan => "Plan",
            Self::Verification => "verification",
            Self::CocoGuide => "coco-guide",
        }
    }

    /// All built-in variants, in display order.
    pub const ALL: &'static [SubagentType] = &[
        Self::GeneralPurpose,
        Self::StatusLine,
        Self::Explore,
        Self::Plan,
        Self::Verification,
        Self::CocoGuide,
    ];
}

impl fmt::Display for SubagentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SubagentType {
    type Err = String;

    /// Accept TS canonical case, plus lowercase aliases for `Explore`/`Plan`
    /// and underscore variants for the kebab-case names. Output is always
    /// canonical via `Display`/`as_str`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "general-purpose" | "general_purpose" => Ok(Self::GeneralPurpose),
            "statusline-setup" | "statusline_setup" => Ok(Self::StatusLine),
            "Explore" | "explore" => Ok(Self::Explore),
            "Plan" | "plan" => Ok(Self::Plan),
            "verification" => Ok(Self::Verification),
            "coco-guide" | "coco_guide" => Ok(Self::CocoGuide),
            _ => Err(format!("unknown subagent type: {s}")),
        }
    }
}

impl Serialize for SubagentType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SubagentType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Agent identity — same pattern as ToolId.
/// Serde as flat string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentTypeId {
    Builtin(SubagentType),
    Custom(String),
}

impl fmt::Display for AgentTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(t) => f.write_str(t.as_str()),
            Self::Custom(name) => f.write_str(name),
        }
    }
}

impl FromStr for AgentTypeId {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(SubagentType::from_str(s)
            .map(Self::Builtin)
            .unwrap_or_else(|_| Self::Custom(s.into())))
    }
}

impl Serialize for AgentTypeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AgentTypeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        // Infallible parse — AgentTypeId::from_str always succeeds
        // (unknown strings become Custom variants).
        Ok(s.parse().expect("AgentTypeId::from_str is Infallible"))
    }
}

impl From<SubagentType> for AgentTypeId {
    fn from(t: SubagentType) -> Self {
        Self::Builtin(t)
    }
}

// ── Agent Source ──

/// Where an agent definition came from. Drives precedence when the same
/// `agent_type` is defined in multiple places: later source wins.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AgentSource {
    /// Bundled built-in agents.
    #[default]
    #[serde(rename = "built-in")]
    BuiltIn,
    /// Plugin contribution.
    #[serde(rename = "plugin")]
    Plugin,
    /// User-level agents (`~/.coco/agents/*.md`).
    #[serde(rename = "userSettings")]
    UserSettings,
    /// Project-level agents (`<project>/.coco/agents/*.md`).
    #[serde(rename = "projectSettings")]
    ProjectSettings,
    /// JSON/CLI flag supplied agents.
    #[serde(rename = "flagSettings")]
    FlagSettings,
    /// Managed/policy agents (highest priority).
    #[serde(rename = "policySettings")]
    PolicySettings,
}

impl AgentSource {
    /// Priority for conflict resolution. Higher wins.
    /// Later-source definitions override earlier ones (map-overwrite order).
    pub const fn priority(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::Plugin => 1,
            Self::UserSettings => 2,
            Self::ProjectSettings => 3,
            Self::FlagSettings => 4,
            Self::PolicySettings => 5,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Plugin => "plugin",
            Self::UserSettings => "userSettings",
            Self::ProjectSettings => "projectSettings",
            Self::FlagSettings => "flagSettings",
            Self::PolicySettings => "policySettings",
        }
    }
}

impl fmt::Display for AgentSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Agent Color ──

/// TS `AgentColorName`. Validated set; unknown values are dropped at parse
/// time with a warning so the runtime never sees an invalid color.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentColorName {
    Red,
    Blue,
    Green,
    Yellow,
    Purple,
    Orange,
    Pink,
    Cyan,
}

impl AgentColorName {
    pub const ALL: &'static [AgentColorName] = &[
        Self::Red,
        Self::Blue,
        Self::Green,
        Self::Yellow,
        Self::Purple,
        Self::Orange,
        Self::Pink,
        Self::Cyan,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Purple => "purple",
            Self::Orange => "orange",
            Self::Pink => "pink",
            Self::Cyan => "cyan",
        }
    }
}

impl fmt::Display for AgentColorName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentColorName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "red" => Ok(Self::Red),
            "blue" => Ok(Self::Blue),
            "green" => Ok(Self::Green),
            "yellow" => Ok(Self::Yellow),
            "purple" => Ok(Self::Purple),
            "orange" => Ok(Self::Orange),
            "pink" => Ok(Self::Pink),
            "cyan" => Ok(Self::Cyan),
            other => Err(format!("unknown agent color: {other}")),
        }
    }
}

/// Identity badge for a teammate whose tool needs the leader's approval.
/// Surfaced in the leader's permission prompt so a human reviewing a
/// cross-process worker's request can see WHO is asking. The `color` is
/// the worker's assigned per-teammate palette color (a coco-rs
/// improvement over TS's hardcoded `cyan`); text-surface renderers show
/// the name and carry the color for styled / SDK consumers.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerBadge {
    pub name: String,
    pub color: AgentColorName,
}

// ── Agent Isolation ──

/// Isolation mode for a subagent's execution environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentIsolation {
    /// No isolation — agent shares the parent's working directory.
    #[default]
    None,
    /// Git worktree — agent gets an isolated worktree branch.
    Worktree,
    /// Remote execution via Claude Code Remote (CCR).
    Remote,
}

impl fmt::Display for AgentIsolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Worktree => f.write_str("worktree"),
            Self::Remote => f.write_str("remote"),
        }
    }
}

impl FromStr for AgentIsolation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "worktree" => Ok(Self::Worktree),
            "remote" => Ok(Self::Remote),
            _ => Err(format!("unknown agent isolation mode: {s}")),
        }
    }
}

// ── Agent MCP server spec (string-ref vs inline) ──

/// One entry in `AgentDefinition.mcp_servers`. Mirrors the TS union:
/// either a `string` (reference to an existing MCP server config) or
/// an inline `{name: config}` mapping that stands up a dynamic
/// server scoped to this agent. Inline configs are stored as
/// `serde_json::Value` because the underlying `McpServerConfig`
/// shape lives in `coco-mcp` (a higher layer that depends on
/// `coco-types`); keeping it as opaque JSON avoids a back-edge.
///
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentMcpServerSpec {
    /// Reference an existing MCP server by name (TS `string` arm).
    Name(String),
    /// Inline definition `{name: config}` (TS record arm). The map
    /// always carries exactly one entry — the server name → its
    /// `McpServerConfig` JSON. TS validates this with Zod on load.
    Inline(std::collections::BTreeMap<String, serde_json::Value>),
}

impl AgentMcpServerSpec {
    /// The server name this spec references (string-ref form) or
    /// declares (inline form's first key).
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Name(s) => Some(s.as_str()),
            Self::Inline(map) => map.keys().next().map(String::as_str),
        }
    }

    /// Inline-form config payload (the value side of the single entry).
    /// `None` for string-ref form.
    pub fn inline_config(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Name(_) => None,
            Self::Inline(map) => map.values().next(),
        }
    }

    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Inline(_))
    }
}

// ── Memory Scope ──

/// Scope for agent memory persistence. Controls where MEMORY.md is stored/read.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// User-global memory (~/.coco/MEMORY.md).
    User,
    /// Project-scoped memory (project root MEMORY.md).
    #[default]
    Project,
    /// Local-only memory (not persisted across sessions).
    Local,
}

impl fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Project => f.write_str("project"),
            Self::Local => f.write_str("local"),
        }
    }
}

impl FromStr for MemoryScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            "local" => Ok(Self::Local),
            _ => Err(format!("unknown memory scope: {s}")),
        }
    }
}

// ── Model Inheritance ──

/// Where a model setting was resolved from (for inheritance debugging).
///
/// Precedence: Param > Definition > Parent (highest to lowest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// Explicitly passed as a spawn parameter.
    Param,
    /// Defined in the agent definition file.
    Definition,
    /// Inherited from the parent agent.
    Parent,
}

impl fmt::Display for ModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Param => f.write_str("param"),
            Self::Definition => f.write_str("definition"),
            Self::Parent => f.write_str("parent"),
        }
    }
}

/// Tracks how a model was resolved through the inheritance chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInheritance {
    /// The resolved model identifier.
    pub model: String,
    /// Where the model value was resolved from.
    pub source: ModelSource,
}

// ── Tool allow-list ──

/// An agent's tool allow-list as parsed from frontmatter. The enum
/// distinguishes three states that `Vec<String>` collapsed into one:
///
/// - `Wildcard` — the frontmatter omitted `tools:` or declared
///   `tools: ['*']`. Semantically: the agent sees every registered
///   tool (subject to the deny-list and parent narrowing). TS:
///   `tools === undefined`.
/// - `Explicit(non-empty list)` — the frontmatter declared a finite
///   list. The agent sees only those tools (subject to deny-list /
///   parent narrowing).
/// - `Explicit(vec![])` — the frontmatter explicitly declared
///   `tools: []`. Semantically: **zero tools** (the agent is
///   tool-less). TS `parseAgentToolsFromFrontmatter` returns `[]` for
///   this case so the auto-memory injector at `loadAgentsDir.ts:455`
///   can promote it to `[Read, Edit, Write]` when `memory:` is set.
///   The agent-tool renderer collapses `Explicit(vec![])` back to
///   "All tools" wording, matching TS `getToolsDescription`'s
///   `allowedTools.length > 0` gate.
///
/// TS parity matrix (`utils/markdownConfigLoader.ts:113-126`,
/// `loadAgentsDir.ts:455-479`):
///
/// | Frontmatter        | parsed   | Rust                | Memory injection |
/// |--------------------|----------|---------------------|------------------|
/// | (key absent)       | undef    | `Wildcard`          | skipped          |
/// | `tools: ['*']`     | undef    | `Wildcard`          | skipped          |
/// | `tools: []`        | `[]`     | `Explicit(vec![])`  | runs             |
/// | `tools: [Read]`    | `[Read]` | `Explicit([Read])`  | runs             |
///
/// Representing the distinction in the type system prevents the next
/// refactor from confusing "wildcard" with "no tools".
///
/// Wire shape: a flat `Vec<String>` mirroring TS JSON. `Wildcard` ↔
/// `["*"]`; `Explicit(v)` ↔ `v` (including `[]`). Combined with
/// `#[serde(skip_serializing_if = "ToolAllowList::is_wildcard")]` on
/// `AgentDefinition.allowed_tools`, wildcard agents emit no `tools`
/// key at all — matching TS `tools: undefined` byte-for-byte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolAllowList {
    /// Every registered tool is visible (subject to deny-list +
    /// parent filter).
    #[default]
    Wildcard,
    /// Only these tool names are visible.
    Explicit(Vec<String>),
}

impl ToolAllowList {
    /// Frontmatter-friendly constructor.
    ///
    /// - `['*']` (single-star sentinel) → [`Self::Wildcard`].
    /// - `[]` (empty list explicitly declared in YAML) →
    ///   [`Self::Explicit`]`(vec![])`. **Distinct from `Wildcard`**:
    ///   TS `parseAgentToolsFromFrontmatter` returns `[]` for
    ///   `tools: []` so the auto-memory injector
    ///   (`loadAgentsDir.ts:455`) can promote it to `[Read, Edit,
    ///   Write]` when `memory:` is set. Preserving the empty array
    ///   here keeps that semantics intact.
    /// - Otherwise → [`Self::Explicit`]`(items)`.
    ///
    /// "Key absent" (whole field omitted from the YAML) is *not*
    /// distinguishable at this entry point — callers must use
    /// `.unwrap_or_default()` on the `Option<Vec<String>>` returned by
    /// the frontmatter reader, which yields [`Self::Wildcard`].
    pub fn from_frontmatter(items: Vec<String>) -> Self {
        if items.len() == 1 && items[0].trim() == "*" {
            return Self::Wildcard;
        }
        Self::Explicit(items)
    }

    /// Returns `true` if every registered tool is visible.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, Self::Wildcard)
    }

    /// Returns the explicit list, or `None` for wildcard.
    pub fn as_explicit(&self) -> Option<&[String]> {
        match self {
            Self::Wildcard => None,
            Self::Explicit(v) => Some(v),
        }
    }

    /// Mutable access to the explicit list. Returns `None` for wildcard
    /// — callers that want to inject tools (e.g. memory auto-injection)
    /// must check `is_wildcard()` and skip rather than coerce.
    pub fn as_explicit_mut(&mut self) -> Option<&mut Vec<String>> {
        match self {
            Self::Wildcard => None,
            Self::Explicit(v) => Some(v),
        }
    }
}

impl Serialize for ToolAllowList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        match self {
            Self::Wildcard => {
                // Wire form `["*"]` for the rare case the field is
                // emitted at all. With
                // `#[serde(skip_serializing_if = "is_wildcard")]` on
                // the field, this branch is normally unreachable —
                // kept so direct `serde_json::to_value(&list)` calls
                // still produce a valid TS-compatible shape.
                let mut s = serializer.serialize_seq(Some(1))?;
                s.serialize_element("*")?;
                s.end()
            }
            Self::Explicit(items) => items.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ToolAllowList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = <Vec<String>>::deserialize(deserializer)?;
        Ok(Self::from_frontmatter(items))
    }
}

// ── Agent Definition ──

/// Complete agent definition — the declarative spec for a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Agent type identity (TS `agentType`).
    pub agent_type: AgentTypeId,

    /// Human-readable display name.
    #[serde(default)]
    pub name: String,

    /// User-facing summary shown in the AgentTool prompt list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,

    /// Free-form description (kept for back-compat; loaders should mirror it
    /// into `when_to_use` when the source has no separate `whenToUse`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Where the definition came from. Drives precedence when the same
    /// `agent_type` is defined in multiple sources.
    #[serde(default)]
    pub source: AgentSource,

    /// Original markdown filename, for diagnostics and `/agents show`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    /// Source directory (e.g. `<project>/.coco/agents`), for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,

    /// System prompt body (TS markdown body / JSON `prompt`). Distinct from
    /// `initial_prompt`, which is a first-turn user-message prefix.
    ///
    /// Built-in agents leave this empty and provide a renderer that produces
    /// the prompt at spawn time using the parent context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Thinking / effort selector for this agent. Typed
    /// [`ReasoningEffort`] — the **only** legal values are enum
    /// variants. Used as the discriminator in
    /// `ModelInfo.supported_thinking_levels.find(|l| l.effort == effort)`
    /// at session-runtime resolution time
    /// (`app/cli/src/session_runtime.rs::thinking_level_for_effort_from`).
    /// The lookup is model-relative — the same `ReasoningEffort::High`
    /// resolves to a different `ThinkingLevel` (budget / options) on
    /// each model, but the key itself is enum-typed.
    ///
    /// **Why not String + numeric form**: TS `parseEffortValue` over-
    /// loaded the field with numeric budget values. Rust's downstream
    /// (`thinking_level_for_effort_from`) takes `ReasoningEffort`
    /// directly — there is no consumer for numeric input, so accepting
    /// it produced silently-dropped values. Rejected at the frontmatter
    /// parser with a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,

    /// Cache-identical tool schema prefixes for stable cache keys.
    #[serde(default)]
    pub use_exact_tools: bool,

    /// Concrete model override for this agent, in `provider/model_id`
    /// format. Use `"inherit"` for parent's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Explicit `ModelRole` declaration. When present, the spawn-time
    /// resolver uses this role as the source of truth — overriding the
    /// default `subagent_type → ModelRole` mapping. Lets a custom `.md`
    /// agent declare e.g. `model_role: explore` to ride on the user's
    /// `~/.coco/config.json` Explore role mapping rather than the
    /// generic Subagent role.
    ///
    /// No TS equivalent — TS historically used model alias strings
    /// directly without a role indirection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_role: Option<ModelRole>,

    /// Isolation mode for the agent's execution environment.
    #[serde(default)]
    pub isolation: AgentIsolation,

    /// Memory persistence scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<MemoryScope>,

    /// Per-agent MCP server specs to connect. Each entry is either a
    /// string-reference to an existing server name or an inline
    /// `{name: config}` mapping that creates a fresh dynamic server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<AgentMcpServerSpec>,

    /// Starting prompt prefix for the first user turn (TS `initialPrompt`).
    /// **Not** the system prompt — see `system_prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,

    /// Maximum turns before the agent should stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<i32>,

    /// Tools this agent is not allowed to use (deny-list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,

    /// Tools this agent is explicitly allowed to use. The enum
    /// distinguishes `Wildcard` (every registered tool, mirroring TS
    /// `tools: undefined` and `tools: ['*']`) from `Explicit(list)` so
    /// the inject site for auto-memory tools can skip wildcards rather
    /// than silently coerce them into `Explicit([Read, Edit, Write])`.
    /// Skipped at serialize time when `Wildcard` so the JSON byte-matches
    /// TS `tools: undefined`.
    #[serde(default, skip_serializing_if = "ToolAllowList::is_wildcard")]
    pub allowed_tools: ToolAllowList,

    /// System prompt / identity override (legacy — prefer `system_prompt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,

    /// Validated UI color for this agent type (TS `color`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<AgentColorName>,

    /// Skill names to preload when this agent starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,

    /// Whether this agent always runs in the background.
    #[serde(default)]
    pub background: bool,

    /// Permission mode override for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,

    /// MCP server name patterns required (agent disabled if not configured).
    /// Separate from `mcp_servers` which are servers to connect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_mcp_servers: Vec<String>,

    /// Omit CLAUDE.md context for read-only agents.
    #[serde(default)]
    pub omit_claude_md: bool,

    /// Short reminder re-injected at every user turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_system_reminder: Option<String>,

    /// Frontmatter hooks scoped to this agent's lifecycle. Same shape
    /// as `Settings.hooks` (event-keyed map of matcher+handler entries);
    /// stored as opaque `Value` here because parsing into typed
    /// `HookDefinition` lives in `coco_hooks` (avoids a back-edge from
    /// types → hooks). When set, the spawn lifecycle registers these
    /// hooks under the spawned agent's id and clears them at
    /// SubagentStop. TS parity: `loadAgentsDir.ts` reads `hooks` into
    /// the definition; `runAgent.ts:564-575` calls
    /// `registerFrontmatterHooks(setAppState, agentId, definition.hooks, ...)`.
    /// `Null` ⇒ no per-agent hooks.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub hooks: serde_json::Value,

    /// Snapshot timestamp this agent's local memory dir is *behind*.
    /// `None` ⇒ either no snapshot is published for this agent or the
    /// local memory is already up-to-date. Populated by the loader
    /// (`AgentDefinitionStore`) at load time via a caller-supplied
    /// inspector closure that consults
    /// [`coco_memory::agent_memory_snapshot::check_agent_memory_snapshot`].
    /// TS parity: `pendingSnapshotUpdate?: string` in
    /// `loadAgentsDir.ts` — set when `checkAgentMemorySnapshot` returns
    /// `prompt-update`. Consumed by `/agents show` to flag drifted
    /// agents and (future) by an interactive resync prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_snapshot_update: Option<String>,
}

impl Default for AgentDefinition {
    fn default() -> Self {
        Self {
            agent_type: AgentTypeId::Custom("default".into()),
            name: String::new(),
            when_to_use: None,
            description: None,
            source: AgentSource::BuiltIn,
            filename: None,
            base_dir: None,
            system_prompt: None,
            effort: None,
            use_exact_tools: false,
            model: None,
            model_role: None,
            isolation: AgentIsolation::None,
            memory_scope: None,
            mcp_servers: Vec::new(),
            initial_prompt: None,
            max_turns: None,
            disallowed_tools: Vec::new(),
            allowed_tools: ToolAllowList::Wildcard,
            identity: None,
            color: None,
            skills: Vec::new(),
            background: false,
            permission_mode: None,
            required_mcp_servers: Vec::new(),
            omit_claude_md: false,
            critical_system_reminder: None,
            hooks: serde_json::Value::Null,
            pending_snapshot_update: None,
        }
    }
}

#[cfg(test)]
#[path = "agent.test.rs"]
mod tests;
