use serde::Deserialize;
use serde::Serialize;

use crate::ThinkingLevel;

/// Where a command can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAvailability {
    ClaudeAi,
    Console,
}

/// How a command was loaded.
/// Payload-carrying variants (`Plugin { name }`, `Mcp { server_name }`)
/// ensure source and attribution can never disagree. This replaces the
/// older `loaded_from + plugin_name` dual-field layout, which allowed
/// nonsensical states (e.g. `loaded_from = Builtin` paired with
/// `plugin_name = Some(...)`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandSource {
    /// Hardcoded built-in slash command (e.g. `/help`, `/clear`).
    Builtin,
    /// Compiled-in bundled skill.
    Bundled,
    /// User-scope on-disk skill (`config home/skills/`).
    User,
    /// Project-scope on-disk skill (`project config dir/skills/`).
    Project,
    /// Enterprise/policy-managed skill.
    Managed,
    /// On-disk skill directory (general SKILL.md catch-all).
    Skills,
    /// Deprecated legacy flat-`.md` path. Not used by project skill discovery.
    CommandsDeprecated,
    /// Plugin-provided skill or command. Carries the contributing
    /// plugin's manifest name so the UI can render
    /// `(plugin-name) text` annotations without a parallel field.
    Plugin { name: String },
    /// MCP-server-provided skill. Carries the originating server name
    /// so `/skills` can list contributing servers without a parallel
    /// lookup table.
    Mcp { server_name: String },
}

impl CommandSource {
    /// Wire string used by TS Skill tool listing / analytics. Returns
    /// only the discriminant — payload (plugin / server name) is not
    /// part of this string.
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandSource::Skills => "skills",
            CommandSource::Plugin { .. } => "plugin",
            CommandSource::Bundled => "bundled",
            CommandSource::Mcp { .. } => "mcp",
            CommandSource::User => "userSettings",
            CommandSource::Project => "projectSettings",
            CommandSource::Managed => "policySettings",
            CommandSource::Builtin => "builtin",
            CommandSource::CommandsDeprecated => "commands_DEPRECATED",
        }
    }

    /// Plugin attribution iff this source is [`CommandSource::Plugin`].
    /// Convenience for sites that previously used the dual-field
    /// `plugin_name: Option<String>` shape.
    pub fn plugin_name(&self) -> Option<&str> {
        match self {
            CommandSource::Plugin { name } => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Provenance badge for a skill produced by the skill-learning loop, rendered
/// as a `/`-popup suffix so a user can tell an auto-generated skill apart from
/// a hand-written one. Orthogonal to [`CommandSource`] (which records the
/// *scope* the file lives in — an agent skill's scope is still the user config
/// home): this axis records *who authored it and whether it is proven*.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillProvenanceBadge {
    /// Agent-created and still quarantined — the Curator has not promoted it,
    /// so the model cannot auto-invoke it yet (reachable only via user `/name`).
    Learning,
    /// Agent-created and promoted by the Curator to model-invocable.
    Learned,
}

/// Interactive session kinds in which a slash command may run.
///
/// Commands default to the primary session. Sidechat support is opt-in so a
/// newly registered builtin, plugin command, or skill cannot accidentally
/// mutate the hidden parent or escape the sidechat's restricted surface.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandSessionScope {
    #[default]
    PrimaryOnly,
    PrimaryAndSideChat,
}

impl SlashCommandSessionScope {
    pub fn supports_side_chat(self) -> bool {
        matches!(self, Self::PrimaryAndSideChat)
    }
}

impl SkillProvenanceBadge {
    /// The `/`-popup suffix tag (no parentheses).
    pub fn suffix_tag(self) -> &'static str {
        match self {
            Self::Learning => "learning",
            Self::Learned => "learned",
        }
    }
}

/// Common fields for all commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandBase {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub availability: Vec<CommandAvailability>,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default)]
    pub argument_kind: CommandArgumentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub user_invocable: bool,
    #[serde(default)]
    pub is_sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_from: Option<CommandSource>,
    /// Skill-learning provenance badge (agent-created skills only; `None` for
    /// everything else). Drives the `(learning)` / `(learned)` `/`-popup suffix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_badge: Option<SkillProvenanceBadge>,
    /// Safety classification for remote/bridge mode filtering.
    #[serde(default)]
    pub safety: CommandSafety,
    /// Whether non-interactive (SDK/headless) mode is supported.
    #[serde(default)]
    pub supports_non_interactive: bool,
    /// Whether the command may execute from an ephemeral sidechat child.
    #[serde(default)]
    pub session_scope: SlashCommandSessionScope,
}

/// Safety classification for remote/bridge filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommandSafety {
    /// Safe in all contexts (remote, bridge, local).
    AlwaysSafe,
    /// Safe for bridge mode (mobile/web clients) but not remote.
    BridgeSafe,
    /// Only safe when running locally in the terminal.
    #[default]
    LocalOnly,
}

impl CommandSafety {
    /// Whether a command with this safety level is allowed in the given context.
    pub fn permits(self, required: CommandSafety) -> bool {
        match required {
            CommandSafety::LocalOnly => true,
            CommandSafety::BridgeSafe => {
                matches!(self, CommandSafety::AlwaysSafe | CommandSafety::BridgeSafe)
            }
            CommandSafety::AlwaysSafe => self == CommandSafety::AlwaysSafe,
        }
    }
}

/// Command execution type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandType {
    /// Expands to a model prompt (skills).
    Prompt(PromptCommandData),
    /// Executes locally, returns text.
    Local(LocalCommandData),
    /// Opens a TUI overlay/modal.
    LocalOverlay(LocalCommandData),
}

impl CommandType {
    /// Discriminant-only projection used by UI snapshots
    /// ([`SlashCommandInfo`]) and other call sites that need a
    /// [`Copy`] tag without the per-variant payload. Centralising the
    /// match here keeps the two enums from drifting — adding a new
    /// [`CommandType`] variant forces an update to [`CommandTypeTag`]
    /// (the match is exhaustive and won't compile otherwise).
    pub const fn tag(&self) -> CommandTypeTag {
        match self {
            CommandType::Prompt(_) => CommandTypeTag::Prompt,
            CommandType::Local(_) => CommandTypeTag::Local,
            CommandType::LocalOverlay(_) => CommandTypeTag::LocalOverlay,
        }
    }
}

/// Tag-only projection of [`CommandType`]. Implements [`Copy`] so the
/// UI snapshot ([`SlashCommandInfo`]) and the autocomplete ranker can
/// pass it around without cloning.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTypeTag {
    /// Expands into a model prompt.
    Prompt,
    /// Executes locally and returns text.
    #[default]
    Local,
    /// Opens a TUI overlay.
    LocalOverlay,
}

/// Typed command argument shape used by UI completion and submit semantics.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandArgumentKind {
    /// Command takes no argument and may execute immediately after selection.
    #[default]
    None,
    /// Command accepts arbitrary text; UI shows a hint but no typed completion.
    FreeText,
    /// Command expects a file path.
    FilePath,
    /// Command expects a directory path.
    DirectoryPath,
    /// Command expects a saved session id.
    SessionId,
}

impl CommandArgumentKind {
    pub const fn accepts_arguments(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Context for prompt command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CommandContext {
    #[default]
    Inline,
    Fork,
}

/// Data for a prompt-type command (sent to LLM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCommandData {
    pub progress_message: String,
    #[serde(default)]
    pub content_length: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub context: CommandContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    /// Hook config — deserialized by coco-hooks, not typed here (avoids L1→L4 dep).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<serde_json::Value>,
}

/// Data for a local-type command (executed locally).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalCommandData {
    /// Module path or identifier for the local command handler.
    pub handler: String,
}

/// UI-facing projection of a slash command. The TUI receives a `Vec` of
/// these at startup (and again after `/reload-plugins`) so the
/// autocomplete popup and command palette can render and rank without
/// reaching into [`CommandBase`] every time.
/// Lives in `coco-types` (rather than `coco-tui`) so it can travel on a
/// [`crate::TuiOnlyEvent`] variant — events are the only path between
/// the agent driver and the TUI, and event payload types must be
/// foundation-layer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlashCommandInfo {
    /// Canonical command name without the leading `/`.
    pub name: String,
    /// Short description shown dimmed in the popup. `None` when the
    /// source command registered without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Alternate names that also match this command. Searched by the
    /// ranker so `/cls` finds `/clear` when `cls` is an alias.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Hint string rendered next to the description when the command
    /// takes arguments (e.g. `"<file>"` for `/add-dir`). Mirrors
    /// [`CommandBase::argument_hint`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Typed argument shape. Drives argument completion and whether Enter
    /// executes immediately after accepting a slash suggestion.
    #[serde(default)]
    pub argument_kind: CommandArgumentKind,
    /// Where this command came from. Drives empty-query source grouping
    /// in the `/` popup and the `(user)` / `(project)` / `(plugin)`
    /// suffix on descriptions (`formatDescriptionWithSource`, empty-input
    /// grouping in `generateCommandSuggestions`). Plugin / MCP attribution
    /// rides on the `Plugin { name }` / `Mcp { server_name }` variants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CommandSource>,
    /// Skill-learning provenance badge (agent-created skills only). Rendered as
    /// a `(learning)` / `(learned)` suffix in the `/` popup, taking precedence
    /// over the [`Self::source`] suffix (an agent skill's scope is always the
    /// user config home, so `(user)` would be redundant). Mirrors
    /// [`CommandBase::skill_badge`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<SkillProvenanceBadge>,
    /// Execution kind. The empty-query ranker treats only
    /// [`CommandTypeTag::Prompt`] entries as skills eligible for the
    /// "recently used" section — builtin local commands always sit in
    /// the builtin bucket (`cmd.type === 'prompt'` filtering).
    /// Derived from the source [`CommandType`] via
    /// [`CommandType::tag`] at snapshot time; the projection is
    /// centralised there so the enum can't drift.
    #[serde(default)]
    pub kind: CommandTypeTag,
    /// Recency-decayed usage score precomputed at snapshot time.
    /// Higher means "used more recently and/or more often".
    /// `getSkillUsageScore` in `utils/suggestions/skillUsageTracking.ts`
    /// — same 7-day half-life with a 0.1 recency floor.
    /// Embedded in the snapshot so the TUI ranker never touches disk
    /// on the hot popup path. Updated naturally at the existing
    /// snapshot-refresh moments (session start, `/reload-plugins`).
    /// Intra-session staleness is acceptable — the rank only governs
    /// which skills float to the top of the empty-query view; users
    /// pick by name regardless.
    #[serde(default)]
    pub usage_score: f64,
    /// Interactive session scope used by the TUI to project the command list.
    #[serde(default)]
    pub session_scope: SlashCommandSessionScope,
}
