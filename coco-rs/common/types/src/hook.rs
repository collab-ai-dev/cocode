use serde::Deserialize;
use serde::Serialize;
use strum::Display;
use strum::IntoStaticStr;

/// 27 hook event types matching TS `HOOK_EVENTS`
/// (`src/entrypoints/sdk/coreSchemas.ts:355-383`).
///
/// Wire format is **PascalCase** (e.g. `"PreToolUse"`) — identical to
/// TS settings.json keys. Variant names serialize as-is via serde
/// default and strum default; do not add `rename_all`.
///
/// `#[non_exhaustive]` so future TS additions can land without
/// breaking match exhaustiveness in downstream crates.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, IntoStaticStr,
)]
#[non_exhaustive]
pub enum HookEventType {
    // Tool lifecycle
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    // Session lifecycle
    SessionStart,
    SessionEnd,
    Setup,
    Stop,
    StopFailure,
    // Subagent lifecycle
    SubagentStart,
    SubagentStop,
    // User interaction
    UserPromptSubmit,
    PermissionRequest,
    PermissionDenied,
    Notification,
    Elicitation,
    ElicitationResult,
    // Compaction
    PreCompact,
    PostCompact,
    // Task lifecycle
    TeammateIdle,
    TaskCreated,
    TaskCompleted,
    // Config & environment
    ConfigChange,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
    // Worktree
    WorktreeCreate,
    WorktreeRemove,
}

impl HookEventType {
    /// Wire-format identifier for this event (TS `HOOK_EVENTS` literal,
    /// e.g. `"PreToolUse"`). Backed by the strum-derived
    /// `IntoStaticStr` impl — single source of truth, no duplication.
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Scope that determines hook priority ordering.
///
/// Higher-priority scopes override lower ones. Ordering mirrors the TS
/// settings layering (`utils/settings/`): Policy is enterprise-managed
/// and overrides everything user-set; Session is the most-specific
/// runtime entry; Plugin and Builtin are the broadest defaults.
///
/// Numeric ordering on the wire (Ord/PartialOrd) is preserved so
/// existing code that sorts by `cmp` keeps working — variants are
/// listed in ascending priority.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum HookScope {
    /// Builtin hooks (registered in code at startup, lowest priority).
    Builtin = 0,
    /// Plugin-contributed hooks via `PLUGIN.toml`.
    Plugin = 1,
    /// User-level hooks from `~/.coco/settings.json`.
    #[default]
    User = 2,
    /// Project-level hooks from `.coco/settings.json` in cwd.
    Project = 3,
    /// Local (machine-specific) overrides from `.coco/settings.local.json`.
    Local = 4,
    /// Session-specific hooks (registered programmatically at runtime).
    Session = 5,
    /// Enterprise policy hooks — override everything else (TS
    /// `policySettings` is the highest-precedence settings source).
    Policy = 6,
}

/// Outcome of hook execution.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOutcome {
    Success,
    Blocking,
    NonBlockingError,
    Cancelled,
}

// `HookResult` (which embeds `Option<Message>`) lives in `coco-messages`;
// hook protocol enums (HookEventType / HookOutcome / HookScope) stay here.
