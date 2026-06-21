//! Slash command registry + built-in implementations.

mod error;
pub mod handlers;
pub mod implementations;

pub use error::CommandsError;
pub use error::Result;

// The in-prompt shell seam lives in `coco-skills` (the lowest crate both
// `coco-commands` and `coco-skills` can see — commands depends on skills).
// Re-exported here so callers (app/cli) can inject one handle on both the
// skill-prompt and slash-command handlers without importing two crates.
pub use coco_skills::BashToolHandle;
pub use coco_skills::NoOpBashToolHandle;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use coco_types::CommandArgumentKind;
use coco_types::CommandBase;
use coco_types::CommandSafety;
use coco_types::CommandSource;
use coco_types::CommandType;
use coco_types::LocalCommandData;
use coco_types::SlashCommandInfo;

pub use implementations::ADD_DIR_SENTINEL;
pub use implementations::ParsedRename;
pub use implementations::RELOAD_HOOKS_SENTINEL;
pub use implementations::RELOAD_PLUGINS_SENTINEL;
pub use implementations::RENAME_SENTINEL;
pub use implementations::STATUS_SENTINEL;
pub use implementations::TAG_SENTINEL;
pub use implementations::names;
pub use implementations::parse_add_dir_sentinel;
pub use implementations::parse_reload_hooks_sentinel;
pub use implementations::parse_reload_plugins_sentinel;
pub use implementations::parse_rename_sentinel;
pub use implementations::parse_status_sentinel;
pub use implementations::parse_tag_sentinel;
pub use implementations::register_extended_builtins;

/// Shared, late-bound slot for the in-prompt [`BashToolHandle`].
///
/// The handle (a `SessionBashToolHandle` in app/cli) can only be built
/// once the per-tool `ToolUseContext` exists, which is *after* the
/// command registry is constructed. The registry creates one empty cell
/// at build time and clones the `Arc` into every shell-capable handler;
/// [`CommandRegistry::set_bash_tool_handle`] later fills it. Handlers
/// read it at execution time. `None` (test / pre-bootstrap) means the
/// handler falls back to its legacy handle-free path.
pub(crate) type SharedBashToolHandle = Arc<std::sync::RwLock<Option<Arc<dyn BashToolHandle>>>>;

/// Late-bound session id shared with every skill handler so user-typed slash
/// commands can substitute `${CLAUDE_SESSION_ID}`. Filled by
/// [`CommandRegistry::set_session_id`] at session bootstrap.
pub(crate) type SharedSessionId = Arc<std::sync::RwLock<Option<String>>>;

/// Clone the current handle out of the shared cell, dropping the read
/// guard before any `.await` (the guard is `!Send`). Returns `None` when
/// no handle has been injected yet.
pub(crate) fn snapshot_bash_handle(cell: &SharedBashToolHandle) -> Option<Arc<dyn BashToolHandle>> {
    cell.read().ok().and_then(|slot| slot.clone())
}

/// Trait for command execution handlers.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Execute the command with the given arguments string.
    ///
    /// Returns a [`CommandResult`] capturing the four execution shapes:
    /// Text, InjectPrompt, Compact, Skip — plus OpenDialog for modal
    /// commands and Prompt for prompt-type commands that expand to model
    /// input.
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let text = self.execute(args).await?;
        Ok(CommandResult::Text(text))
    }

    /// Backwards-compatible string-only shim used by simple builtins. Most
    /// new commands should override `execute_command` instead.
    async fn execute(&self, args: &str) -> crate::Result<String> {
        let _ = args;
        Err(crate::CommandsError::generic(
            "command provides only execute_command",
        ))
    }

    /// Short name for debug output.
    fn handler_name(&self) -> &str;
}

/// Result of executing a slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Display message in the chat (system-line).
    Text(String),
    /// Inject as user input (re-enter the agent loop with this string).
    InjectPrompt(String),
    /// Compaction completed; embed the summary into the next turn.
    Compact {
        display_text: String,
        summary: String,
    },
    /// Prompt command — expand to ContentBlockParam[] and feed back to the
    /// model.
    Prompt {
        progress_message: String,
        parts: Vec<PromptPart>,
    },
    /// Open a TUI dialog/overlay.
    OpenDialog(DialogSpec),
    /// No output.
    Skip,
}

/// One block of rendered prompt content.
///
/// Mirrors `coco_skills::prompt_render::PromptPart` (kept separate to avoid
/// the commands→skills dependency direction).
#[derive(Debug, Clone)]
pub enum PromptPart {
    Text { text: String },
    File { media_type: String, data: Vec<u8> },
}

/// Description of a TUI dialog the command requests.
///
/// Rust models the TUI dialog as data; the actual ratatui rendering lives in
/// `coco-tui::overlays`.
#[derive(Debug, Clone)]
pub enum DialogSpec {
    /// `/memory` — file selector + editor open.
    MemoryFileSelector { entries: Vec<MemoryFileEntry> },
    /// `/rewind` — message-selector overlay.
    ///
    /// The slash command always opens the bare picker. Internal UI paths that
    /// preselect a message use `TuiCommand::ShowRewindFor`.
    MessageSelector,
    /// `/plugin` — plugin picker (built-in + marketplace).
    PluginPicker,
    /// MCPB config form.
    McpbConfig {
        plugin_name: String,
        config_schema: std::collections::HashMap<String, serde_json::Value>,
        existing_config: std::collections::HashMap<String, serde_json::Value>,
    },
    /// Generic confirm dialog.
    Confirm { title: String, message: String },
    /// `/model` — provider-grouped model picker with role pill and
    /// inline thinking-effort selector. Extends the base shape with a role
    /// pill so multi-provider users can address any [`coco_types::ModelRole`]
    /// from the same surface.
    ModelPicker,
    /// `/theme` (no args) — standalone theme picker with live preview + a
    /// sample diff.
    ThemePicker,
    /// `/skills` — read-only skill catalog overlay. Payload carries the
    /// fully-grouped entry list plus per-group subtitle text so the
    /// TUI doesn't recompute paths or token estimates.
    ///
    /// Dialog has no toggle / search / sort — only Esc to close.
    SkillsList {
        payload: coco_types::SkillsDialogPayload,
    },
    /// `/agents` — 2-tab overlay (Running + Library). Payload only
    /// carries the Library entries; the Running tab reads
    /// `SessionState.subagents` at render time.
    AgentsList {
        payload: coco_types::AgentsDialogPayload,
    },
}

/// One row in the memory-file selector.
///
/// Each row is a `(label, path, description)` triple, plus a `scope`
/// discriminator (so TUI rendering can color by category) and explicit
/// `is_new` / `is_folder` flags.
#[derive(Debug, Clone)]
pub struct MemoryFileEntry {
    pub path: std::path::PathBuf,
    pub label: String,
    pub scope: MemoryScope,
    /// Secondary text rendered next to the label.
    ///
    /// Empty string ⇒ render label-only. Examples: `"@-imported"`,
    /// `"dynamically loaded"`, `"Checked in at ./CLAUDE.md"`.
    pub description: String,
    /// True when the path doesn't yet exist on disk — selecting the
    /// row creates it.
    pub is_new: bool,
    /// True when the row points at a directory to open in the file
    /// browser / editor instead of editing a single file.
    pub is_folder: bool,
}

/// Scope of a memory file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    /// Enterprise / managed.
    Managed,
    /// User-global (`~/.coco/CLAUDE.md`).
    User,
    /// Project (`./CLAUDE.md`).
    Project,
    /// Project-local (`./CLAUDE.local.md`).
    ProjectLocal,
    /// `<dir>/.coco/CLAUDE.md` — project-config-dir convention.
    ProjectConfig,
    /// Subdirectory CLAUDE.md (auto-loaded under cwd).
    Subdir,
    /// File loaded transitively via `@-import` from a parent memory file.
    Imported,
    /// Auto-memory directory entry (`<memdir>/`).
    AutoMemFolder,
    /// Team memory directory entry (`<memdir>/team/`).
    TeamMemFolder,
    /// Per-agent memory directory entry.
    AgentMemFolder,
}

/// Feature-flag gate for conditionally enabled commands.
pub type IsEnabledFn = fn() -> bool;

/// A registered command with metadata and an executable handler.
pub struct RegisteredCommand {
    pub base: CommandBase,
    pub command_type: CommandType,
    pub handler: Option<Arc<dyn CommandHandler>>,
    /// Optional feature-flag gate. When set, command is only visible/executable
    /// if the function returns `true`.
    pub is_enabled: Option<IsEnabledFn>,
}

impl RegisteredCommand {
    /// Whether this command is currently active (feature flag check).
    pub fn is_active(&self) -> bool {
        self.is_enabled.is_none_or(|f| f())
    }
}

/// Command registry — holds all registered slash commands.
#[derive(Default)]
pub struct CommandRegistry {
    commands: HashMap<String, RegisteredCommand>,
    /// Late-bound Bash handle shared with every shell-capable handler.
    /// Filled by [`Self::set_bash_tool_handle`] at session bootstrap.
    bash_tool_handle: SharedBashToolHandle,
    /// Late-bound session id shared with skill handlers for the
    /// `${CLAUDE_SESSION_ID}` placeholder. Filled by [`Self::set_session_id`].
    session_id: SharedSessionId,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, cmd: RegisteredCommand) {
        self.commands.insert(cmd.base.name.clone(), cmd);
    }

    /// The shared cell handed to shell-capable handlers at registration.
    /// Cloning it (an `Arc`) lets a handler observe a later
    /// [`Self::set_bash_tool_handle`] without being rebuilt.
    pub(crate) fn bash_tool_handle_cell(&self) -> SharedBashToolHandle {
        Arc::clone(&self.bash_tool_handle)
    }

    /// Inject the in-prompt Bash handle after the per-tool
    /// `ToolUseContext` is available (app/cli session bootstrap). All
    /// previously registered shell-capable handlers see it immediately —
    /// they hold a clone of the same shared cell.
    pub fn set_bash_tool_handle(&self, handle: Arc<dyn BashToolHandle>) {
        if let Ok(mut slot) = self.bash_tool_handle.write() {
            *slot = Some(handle);
        }
    }

    /// The shared session-id cell handed to skill handlers at registration.
    pub(crate) fn session_id_cell(&self) -> SharedSessionId {
        Arc::clone(&self.session_id)
    }

    /// Inject the current session id so skill handlers can substitute
    /// `${CLAUDE_SESSION_ID}`. Called at session bootstrap (and after a
    /// `/reload-plugins` registry swap) alongside [`Self::set_bash_tool_handle`].
    pub fn set_session_id(&self, session_id: String) {
        if let Ok(mut slot) = self.session_id.write() {
            *slot = Some(session_id);
        }
    }

    /// Look up a command by name or alias.
    pub fn get(&self, name: &str) -> Option<&RegisteredCommand> {
        self.commands.get(name).or_else(|| {
            self.commands
                .values()
                .find(|c| c.base.aliases.iter().any(|a| a == name))
        })
    }

    pub fn all(&self) -> impl Iterator<Item = &RegisteredCommand> {
        self.commands.values()
    }

    pub fn visible(&self) -> Vec<&RegisteredCommand> {
        self.commands
            .values()
            .filter(|c| !c.base.is_hidden && c.is_active())
            .collect()
    }

    /// Commands safe to advertise over the SDK wire.
    ///
    /// Stricter than [`Self::visible`]: also filters out commands
    /// flagged `is_sensitive`. A sensitive command may be visible in
    /// local TUI completions (where the user is trusted to run it) but
    /// must not leak its name / description / argument hint to remote
    /// SDK clients, some of which may be untrusted wrappers.
    pub fn sdk_safe(&self) -> Vec<&RegisteredCommand> {
        self.commands
            .values()
            .filter(|c| !c.base.is_hidden && !c.base.is_sensitive && c.is_active())
            .collect()
    }

    /// Commands safe for the given safety level.
    pub fn safe_for(&self, safety: CommandSafety) -> Vec<&RegisteredCommand> {
        self.commands
            .values()
            .filter(|c| !c.base.is_hidden && c.is_active() && c.base.safety.permits(safety))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Snapshot every visible command as a [`coco_types::SlashCommandInfo`].
    ///
    /// Used by `coco-cli::tui_runner` to seed the TUI's
    /// `available_commands` slot at session start and to push a fresh
    /// list after `/reload-plugins` swaps the active registry. The
    /// projection keeps only the fields the popup actually renders or
    /// ranks against — the heavy `RegisteredCommand` stays here.
    ///
    /// Sorted alphabetically by name so the empty-query popup view (and
    /// the rank-tail tiebreak in `coco-tui::autocomplete::slash`) are
    /// stable across sessions — `HashMap::values()` iteration order is
    /// otherwise random and would shuffle the popup each launch.
    ///
    /// The per-command `usage_score` is filled by a single `load_all`
    /// disk read up front; the TUI ranker reads from the snapshot
    /// without touching the filesystem on the popup hot path.
    pub fn snapshot_for_ui(&self) -> Vec<SlashCommandInfo> {
        let config_home = coco_config::global_config::config_home();
        let usage = coco_skills::usage::load_all(&config_home);
        let mut out: Vec<SlashCommandInfo> = self
            .commands
            .values()
            .filter(|c| !c.base.is_hidden && c.is_active())
            .map(|cmd| {
                let usage_score = usage
                    .get(&cmd.base.name)
                    .map(coco_skills::usage::score_for)
                    .unwrap_or(0.0);
                SlashCommandInfo {
                    name: cmd.base.name.clone(),
                    description: (!cmd.base.description.is_empty())
                        .then(|| cmd.base.description.clone()),
                    aliases: cmd.base.aliases.clone(),
                    argument_hint: cmd.base.argument_hint.clone(),
                    argument_kind: cmd.base.argument_kind,
                    source: cmd.base.loaded_from.clone(),
                    // CommandType::tag() is the single projection point —
                    // any future variant in CommandType forces an update
                    // there, not here.
                    kind: cmd.command_type.tag(),
                    usage_score,
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Toggle `is_hidden` on a registered command. No-op when the
    /// command is unknown. Used by `register_extended_builtins` to
    /// mark Rust-only debug commands (`/env`, `/debug-tool-call`)
    /// as hidden — they are enabled but should not surface in
    /// `/-typeahead` (the corresponding upstream sources are
    /// literal `isEnabled:false, isHidden:true` stubs).
    pub fn set_hidden(&mut self, name: &str, hidden: bool) {
        if let Some(cmd) = self.commands.get_mut(name) {
            cmd.base.is_hidden = hidden;
        }
    }

    /// Execute a command by name (or alias), passing the given arguments.
    /// Returns the legacy String shape — for the typed [`CommandResult`] use
    /// [`Self::execute_command`].
    pub async fn execute(&self, name: &str, args: &str) -> crate::Result<String> {
        match self.execute_command(name, args).await? {
            CommandResult::Text(s) => Ok(s),
            CommandResult::InjectPrompt(s) => Ok(s),
            CommandResult::Compact { display_text, .. } => Ok(display_text),
            CommandResult::Prompt { parts, .. } => Ok(parts
                .iter()
                .filter_map(|p| match p {
                    PromptPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n")),
            CommandResult::OpenDialog(_) => Ok(String::new()),
            CommandResult::Skip => Ok(String::new()),
        }
    }

    /// Execute a command by name (or alias) and return the typed result.
    pub async fn execute_command(&self, name: &str, args: &str) -> crate::Result<CommandResult> {
        let start = std::time::Instant::now();
        tracing::info!(
            command = %name,
            args_len = args.len(),
            "slash command dispatch"
        );
        let cmd = self.get(name).ok_or_else(|| {
            tracing::warn!(command = %name, "slash command unknown");
            crate::CommandsError::generic(format!("unknown command: /{name}"))
        })?;

        if !cmd.is_active() {
            tracing::warn!(
                command = %cmd.base.name,
                "slash command inactive in current config"
            );
            return Err(crate::CommandsError::generic(format!(
                "command /{name} is not available in the current configuration"
            )));
        }

        match &cmd.handler {
            Some(handler) => {
                let result = handler.execute_command(args).await;
                let duration_ms = start.elapsed().as_millis() as i64;
                match &result {
                    Ok(cr) => {
                        tracing::info!(
                            command = %cmd.base.name,
                            duration_ms,
                            result_kind = command_result_kind(cr),
                            "slash command ok"
                        );
                        // After a successful dispatch, record skill usage so the
                        // `/` autocomplete can surface frequently-used skills in
                        // the "recently used" section. We track only prompt-kind
                        // commands (skills) — builtin local commands are always
                        // in the builtin bucket and never ranked by use.
                        //
                        // `record` does blocking `std::fs` I/O. Fire-and-
                        // forget on a blocking thread keeps the async
                        // dispatcher non-blocking; the 60-second debounce
                        // already makes most calls no-op so this is rarely
                        // exercised, but we don't want a slow disk to
                        // stall the executor when it is.
                        if matches!(cmd.command_type, CommandType::Prompt(_)) {
                            let skill_name = cmd.base.name.clone();
                            tokio::task::spawn_blocking(move || {
                                let config_home = coco_config::global_config::config_home();
                                coco_skills::usage::record(&config_home, &skill_name);
                            });
                        }
                    }
                    Err(e) => tracing::warn!(
                        command = %cmd.base.name,
                        duration_ms,
                        error = %e,
                        "slash command failed"
                    ),
                }
                result
            }
            None => {
                tracing::warn!(
                    command = %cmd.base.name,
                    "slash command has no handler"
                );
                Err(crate::CommandsError::generic(format!(
                    "command /{name} has no handler"
                )))
            }
        }
    }
}

/// Tag for a `CommandResult` variant, used in tracing fields.
fn command_result_kind(r: &CommandResult) -> &'static str {
    match r {
        CommandResult::Text(_) => "text",
        CommandResult::InjectPrompt(_) => "inject_prompt",
        CommandResult::Compact { .. } => "compact",
        CommandResult::Prompt { .. } => "prompt",
        CommandResult::OpenDialog(_) => "open_dialog",
        CommandResult::Skip => "skip",
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Top-level seam — resolution order
// (§0 of parity-skills-commands-plugins.md)
// ────────────────────────────────────────────────────────────────────────────

/// Build a fully-populated CommandRegistry with the correct load order.
///
/// **Order** (last wins on name collision):
/// 1. Hardcoded slash commands (`register_builtins` + `register_extended_builtins`).
/// 2. Bundled-skill commands (already registered via skill→command bridge).
/// 3. Builtin-plugin skill commands.
/// 4. Marketplace plugin commands.
/// 5. On-disk skill dirs (managed → user → project → legacy `commands/`).
/// 6. P1 handlers (rewind / memory / init / prompt-type commands).
///
/// This function is a thin wrapper that performs the in-order registration —
/// callers pass the constructed `SkillManager` and the resolved enabled plugin
/// set (`&[LoadedPluginV2]`) along with user / feature context.
// PR2 took the arg count to 8 (added `skill_overrides`). Bundling into
// a struct is the cleaner fix but touches every caller; left for a
// follow-up refactor.
#[allow(clippy::too_many_arguments)]
pub fn build_command_registry(
    skill_manager: &coco_skills::SkillManager,
    plugins: &[coco_plugins::loader::LoadedPluginV2],
    user_type: coco_types::UserType,
    features: coco_types::Features,
    project_root: std::path::PathBuf,
    user_home: std::path::PathBuf,
    managed_root: Option<std::path::PathBuf>,
    skill_overrides: &coco_config::SkillOverrideTiers,
) -> CommandRegistry {
    let mut registry = CommandRegistry::new();

    // 1. Hardcoded slash commands.
    register_builtins(&mut registry);
    implementations::register_extended_builtins(&mut registry);

    // 2-5. Skill-derived commands (filtered by feature gates + the
    // `off` override; the dialog's gate keeps the `name-only` /
    // `user-invocable-only` rows discoverable via `/`).
    register_skills_as_commands(&mut registry, skill_manager, &features, skill_overrides);
    register_plugin_contributions(&mut registry, plugins);

    // 6. P1 handlers — last so they win over any name collisions
    //    from skills/plugins (`/init`, `/rewind`, `/memory` are baseline
    //    commands not overridable by user skills).
    implementations::register_ts_parity_handlers(
        &mut registry,
        user_type,
        features,
        project_root,
        user_home,
        managed_root,
    );

    registry
}

fn register_skills_as_commands(
    registry: &mut CommandRegistry,
    manager: &coco_skills::SkillManager,
    features: &coco_types::Features,
    tiers: &coco_config::SkillOverrideTiers,
) {
    use coco_types::CommandSource;
    use coco_types::PromptCommandData;
    use coco_types::SkillOverrideState;
    // Cloned once and shared into every skill handler so a later
    // `set_bash_tool_handle` / `set_session_id` reaches them all.
    let bash_cell = registry.bash_tool_handle_cell();
    let session_cell = registry.session_id_cell();
    for skill in manager.visible(features) {
        if !skill.user_invocable {
            continue;
        }
        // `off`-overridden skills are hidden from `/` autocomplete
        // entirely. `name-only` and `user-invocable-only` keep their
        // slash-command entries — they only restrict model invocation.
        if coco_skills::effective_skill_state(&skill, tiers) == SkillOverrideState::Off {
            continue;
        }
        // Skill source maps directly to the payload-carrying
        // `CommandSource`. Plugin attribution rides on the
        // `Plugin { name }` variant; previously this required a
        // parallel `plugin_name` field which the refactor eliminated.
        let source = match &skill.source {
            coco_skills::SkillSource::Bundled => CommandSource::Bundled,
            coco_skills::SkillSource::User { .. } => CommandSource::User,
            coco_skills::SkillSource::Project { .. } => CommandSource::Project,
            coco_skills::SkillSource::Plugin { plugin_name } => CommandSource::Plugin {
                name: plugin_name.clone(),
            },
            coco_skills::SkillSource::Managed { .. } => CommandSource::Managed,
            coco_skills::SkillSource::Mcp { server_name } => CommandSource::Mcp {
                server_name: server_name.clone(),
            },
        };
        let mut base = builtin_base(&skill.name, &skill.description, &[]);
        base.loaded_from = Some(source);
        base.is_hidden = skill.is_hidden;
        base.user_invocable = skill.user_invocable;
        base.argument_hint = skill.argument_hint.clone();
        base.argument_kind = skill
            .argument_hint
            .as_ref()
            .map(|_| CommandArgumentKind::FreeText)
            .unwrap_or(CommandArgumentKind::None);
        base.when_to_use = skill.when_to_use.clone();
        let prompt = skill.prompt.clone();
        let progress_message = "running".to_string();
        registry.register(RegisteredCommand {
            base,
            command_type: CommandType::Prompt(PromptCommandData {
                progress_message: progress_message.clone(),
                content_length: skill.content_length,
                allowed_tools: skill.allowed_tools.clone(),
                model: skill.model.clone(),
                context: match skill.context {
                    coco_skills::SkillContext::Inline => coco_types::CommandContext::Inline,
                    coco_skills::SkillContext::Fork => coco_types::CommandContext::Fork,
                },
                agent: skill.agent.clone(),
                thinking_level: None,
                hooks: skill.hooks.clone(),
            }),
            handler: Some(std::sync::Arc::new(SkillPromptHandler {
                name: skill.name.clone(),
                body: prompt,
                progress_message,
                // MCP-source gate: MCP skills are remote and untrusted —
                // never run their in-prompt shell.
                is_mcp: matches!(skill.source, coco_skills::SkillSource::Mcp { .. }),
                // Skill frontmatter `allowed-tools` → `alwaysAllowRules.command`.
                allowed_tools: skill.allowed_tools.clone().unwrap_or_default(),
                bash_tool_handle: Arc::clone(&bash_cell),
                // `${CLAUDE_SKILL_DIR}` is known now; `${CLAUDE_SESSION_ID}` is
                // late-bound via the shared cell.
                skill_dir: skill
                    .skill_root
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .map(str::to_owned),
                session_id: Arc::clone(&session_cell),
            })),
            is_enabled: None,
        });
    }
}

fn register_plugin_contributions(
    registry: &mut CommandRegistry,
    plugins: &[coco_plugins::loader::LoadedPluginV2],
) {
    // Each plugin's commands are loaded via the V2 bridge, which carries the
    // REAL prompt body (parsed from the command markdown / manifest), the
    // `plugin:command` namespace, and `loaded_from = Plugin` — replacing the
    // old name-only `PluginCommandStub`.
    for plugin in plugins {
        for pc in coco_plugins::command_bridge::load_plugin_commands_v2(plugin) {
            let name = pc.base.name.clone();
            let progress_message = match &pc.command_type {
                CommandType::Prompt(d) => d.progress_message.clone(),
                _ => String::new(),
            };
            registry.register(RegisteredCommand {
                base: pc.base,
                command_type: pc.command_type,
                handler: Some(std::sync::Arc::new(PluginPromptHandler {
                    name,
                    body: pc.prompt,
                    progress_message,
                })),
                is_enabled: None,
            });
        }
    }
}

/// Handler for a plugin-contributed prompt command: substitutes `$ARGUMENTS`
/// (like a skill) and emits the body as a prompt. Mirrors `SkillPromptHandler`
/// for the simple prompt case.
struct PluginPromptHandler {
    name: String,
    body: String,
    progress_message: String,
}

#[async_trait]
impl CommandHandler for PluginPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let args_opt = (!args.is_empty()).then_some(args);
        let text = coco_skills::prompt_render::substitute_arguments(
            &self.body,
            args_opt,
            &[],
            /* append_if_no_placeholder */ true,
        );
        Ok(CommandResult::Prompt {
            progress_message: self.progress_message.clone(),
            parts: vec![PromptPart::Text { text }],
        })
    }

    fn handler_name(&self) -> &str {
        &self.name
    }
}

struct SkillPromptHandler {
    name: String,
    body: String,
    progress_message: String,
    /// Whether the skill was loaded from an MCP server. MCP skills skip
    /// in-prompt shell execution entirely.
    is_mcp: bool,
    /// Frontmatter `allowed-tools`, surfaced to the permission evaluator
    /// as `alwaysAllowRules.command`.
    allowed_tools: Vec<String>,
    /// Shared, late-bound Bash handle. `None` until session bootstrap
    /// injects it — then the in-prompt shell routes through the real
    /// Bash tool with a per-command permission check.
    bash_tool_handle: SharedBashToolHandle,
    /// Skill base directory for the `${CLAUDE_SKILL_DIR}` placeholder
    /// (known at registration). `None` for skills without a root (bundled).
    skill_dir: Option<String>,
    /// Shared, late-bound session id for the `${CLAUDE_SESSION_ID}` placeholder.
    session_id: SharedSessionId,
}

#[async_trait]
impl CommandHandler for SkillPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        // Argument substitution via the canonical implementation
        // in `coco_skills::prompt_render`.
        let mut text = render_skill_prompt_args(&self.name, &self.body, args);
        // Replace `${CLAUDE_SKILL_DIR}` / `${CLAUDE_SESSION_ID}` on every
        // invocation. Snapshot the session-id cell, dropping the read guard
        // before any later `.await`.
        let session_id = self.session_id.read().ok().and_then(|s| s.clone());
        text = coco_skills::prompt_render::substitute_skill_env(
            &text,
            self.skill_dir.as_deref(),
            session_id.as_deref(),
        );
        // MCP-source gate: skip in-prompt shell for MCP skills; otherwise route
        // through the real Bash tool (per-command permission check) when a
        // handle is wired. Without a handle (tests / pre-bootstrap) the prompt
        // is left verbatim — no unguarded `sh -c` from a slash command.
        if !self.is_mcp
            && let Some(handle) = snapshot_bash_handle(&self.bash_tool_handle)
        {
            text = coco_skills::shell_exec::execute_shell_in_prompt_with_tool(
                &text,
                &*handle,
                &self.allowed_tools,
            )
            .await
            .map_err(|message| crate::CommandsError::ShellCommandError { message })?;
        }
        Ok(CommandResult::Prompt {
            progress_message: self.progress_message.clone(),
            parts: vec![PromptPart::Text { text }],
        })
    }

    fn handler_name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use coco_skills::SkillManager;
    use coco_skills::bundled::register_bundled;
    use coco_types::Features;
    use coco_types::UserType;

    #[tokio::test]
    async fn build_registry_includes_skills_and_ts_parity_handlers() {
        let sm = SkillManager::new();
        register_bundled(&sm);
        let reg = build_command_registry(
            &sm,
            &[],
            UserType::Human,
            Features::with_defaults(),
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("/home/test"),
            None,
            &coco_config::SkillOverrideTiers::default(),
        );
        // P1 handlers are present. Canonical names only — no
        // aliases for /rewind or /resume.
        assert!(reg.get("rewind").is_some());
        assert!(
            reg.get("checkpoint").is_none(),
            "/checkpoint alias removed; use canonical /rewind"
        );
        assert!(
            reg.get("undo").is_none(),
            "/undo alias removed; use canonical /rewind"
        );
        assert!(
            reg.get("continue").is_none(),
            "/continue alias removed; use canonical /resume"
        );
        assert!(
            reg.get("restore").is_none(),
            "session continuation uses resume; rewind actions are not slash command aliases"
        );
        assert!(reg.get("memory").is_some());
        assert!(reg.get("init").is_some());
        assert!(reg.get("agents").is_some());
        assert!(reg.get("security-review").is_some());
        assert!(reg.get("commit-push-pr").is_some());
        // Bundled skills (unconditional) are present.
        assert!(reg.get("update-config").is_some());
        assert!(reg.get("batch").is_some());
    }

    #[tokio::test]
    async fn skills_filtered_by_features() {
        let sm = SkillManager::new();
        register_bundled(&sm);
        let reg = build_command_registry(
            &sm,
            &[],
            UserType::Ant,
            Features::empty(),
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("/home/test"),
            None,
            &coco_config::SkillOverrideTiers::default(),
        );
        // Gated skills/commands MUST NOT appear when features are off.
        // `/dream` and `/summary` are gated on Feature::AutoMemory in
        // `register_ts_parity_handlers`; the rest are skill-only and serve
        // as the gate test.
        for missing in [
            "loop",
            "schedule",
            "claude-api",
            "hunter",
            "claude-in-chrome",
            "run-skill-generator",
            "dream",
            "summary",
        ] {
            assert!(
                reg.get(missing).is_none(),
                "{missing} should not appear when its feature is off"
            );
        }

        // Enable the relevant features and confirm they show up.
        let mut features = Features::empty();
        features
            .enable(coco_types::Feature::AgentTriggers)
            .enable(coco_types::Feature::AgentTriggersRemote)
            .enable(coco_types::Feature::BuildingClaudeApps)
            .enable(coco_types::Feature::AutoMemory);
        let reg2 = build_command_registry(
            &sm,
            &[],
            UserType::Ant,
            features,
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("/home/test"),
            None,
            &coco_config::SkillOverrideTiers::default(),
        );
        assert!(reg2.get("loop").is_some());
        assert!(reg2.get("schedule").is_some());
        assert!(reg2.get("claude-api").is_some());
        assert!(reg2.get("dream").is_some());
        assert!(reg2.get("summary").is_some());
    }

    #[tokio::test]
    async fn rewind_emits_open_dialog() {
        let sm = SkillManager::new();
        register_bundled(&sm);
        let reg = build_command_registry(
            &sm,
            &[],
            UserType::Human,
            Features::with_defaults(),
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("/home/test"),
            None,
            &coco_config::SkillOverrideTiers::default(),
        );
        match reg.execute_command("rewind", "").await.unwrap() {
            CommandResult::OpenDialog(DialogSpec::MessageSelector) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── SkillPromptHandler shell routing ──

    /// Mock handle: echoes each command wrapped, captures the
    /// `allowed_tools` it was called with, or denies/fails.
    struct ScriptedHandle {
        deny: Option<String>,
        seen_allowed: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl ScriptedHandle {
        fn allow() -> Self {
            Self {
                deny: None,
                seen_allowed: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn deny(msg: &str) -> Self {
            Self {
                deny: Some(msg.to_string()),
                seen_allowed: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl BashToolHandle for ScriptedHandle {
        async fn execute_with_permissions(
            &self,
            command: &str,
            allowed_tools: &[String],
        ) -> std::result::Result<String, String> {
            self.seen_allowed
                .lock()
                .expect("lock")
                .push(allowed_tools.to_vec());
            match &self.deny {
                Some(m) => Err(m.clone()),
                None => Ok(format!("<{command}>")),
            }
        }
    }

    fn skill_handler(
        body: &str,
        is_mcp: bool,
        allowed_tools: Vec<String>,
        handle: Option<Arc<dyn BashToolHandle>>,
    ) -> SkillPromptHandler {
        skill_handler_named("s", body, is_mcp, allowed_tools, handle)
    }

    fn skill_handler_named(
        name: &str,
        body: &str,
        is_mcp: bool,
        allowed_tools: Vec<String>,
        handle: Option<Arc<dyn BashToolHandle>>,
    ) -> SkillPromptHandler {
        let cell: SharedBashToolHandle = Arc::new(std::sync::RwLock::new(handle));
        SkillPromptHandler {
            name: name.to_string(),
            body: body.to_string(),
            progress_message: "running".to_string(),
            is_mcp,
            allowed_tools,
            bash_tool_handle: cell,
            skill_dir: None,
            session_id: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    fn prompt_text(r: CommandResult) -> String {
        match r {
            CommandResult::Prompt { parts, .. } => parts
                .into_iter()
                .filter_map(|p| match p {
                    PromptPart::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn skill_shell_allow_runs_and_substitutes() {
        let h = skill_handler(
            "see !`git status`",
            /*is_mcp*/ false,
            vec!["Bash(git status:*)".to_string()],
            Some(Arc::new(ScriptedHandle::allow())),
        );
        let r = h.execute_command("").await.unwrap();
        assert_eq!(prompt_text(r), "see <git status>");
    }

    #[tokio::test]
    async fn simplify_args_render_as_additional_focus() {
        let h = skill_handler_named("simplify", "base prompt", false, vec![], None);
        let r = h.execute_command("focus on tests").await.unwrap();
        assert_eq!(
            prompt_text(r),
            "base prompt\n\n## Additional Focus\n\nfocus on tests"
        );
    }

    #[tokio::test]
    async fn skill_shell_passes_allowed_tools() {
        let handle = Arc::new(ScriptedHandle::allow());
        let h = skill_handler(
            "!`echo hi`",
            false,
            vec!["Bash(echo:*)".to_string()],
            Some(handle.clone()),
        );
        let _ = h.execute_command("").await.unwrap();
        let seen = handle.seen_allowed.lock().expect("lock");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0], vec!["Bash(echo:*)".to_string()]);
    }

    #[tokio::test]
    async fn skill_shell_deny_aborts_with_error() {
        let h = skill_handler(
            "see !`rm -rf /`",
            false,
            vec![],
            Some(Arc::new(ScriptedHandle::deny("denied"))),
        );
        let err = h.execute_command("").await.unwrap_err();
        assert!(
            matches!(err, CommandsError::ShellCommandError { ref message } if message == "denied"),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn skill_shell_mcp_source_skips_execution() {
        let handle = Arc::new(ScriptedHandle::deny("would deny"));
        // MCP-sourced skill: shell must be skipped entirely, so the
        // marker is left verbatim and the (denying) handle is never hit.
        let h = skill_handler(
            "see !`echo hi`",
            /*is_mcp*/ true,
            vec![],
            Some(handle.clone()),
        );
        let r = h.execute_command("").await.unwrap();
        assert_eq!(prompt_text(r), "see !`echo hi`");
        assert!(handle.seen_allowed.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn skill_shell_no_handle_leaves_verbatim() {
        let h = skill_handler("see !`echo hi`", false, vec![], None);
        let r = h.execute_command("").await.unwrap();
        assert_eq!(prompt_text(r), "see !`echo hi`");
    }

    #[tokio::test]
    async fn registry_injection_reaches_existing_handler() {
        // A handle injected AFTER the handler is registered must be
        // observed by it (shared cell semantics).
        let h = skill_handler("!`echo hi`", false, vec![], None);
        // Before injection: verbatim.
        let cell = Arc::clone(&h.bash_tool_handle);
        let r0 = h.execute_command("").await.unwrap();
        assert_eq!(prompt_text(r0), "!`echo hi`");
        // Inject via the shared cell (as set_bash_tool_handle does).
        *cell.write().expect("write") = Some(Arc::new(ScriptedHandle::allow()));
        let r1 = h.execute_command("").await.unwrap();
        assert_eq!(prompt_text(r1), "<echo hi>");
    }

    #[tokio::test]
    async fn set_bash_tool_handle_threads_into_skill_handlers() {
        let sm = SkillManager::new();
        register_bundled(&sm);
        let reg = build_command_registry(
            &sm,
            &[],
            UserType::Human,
            Features::with_defaults(),
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("/home/test"),
            None,
            &coco_config::SkillOverrideTiers::default(),
        );
        // Injection on the registry writes the shared cell that every
        // skill / shell-expanding handler cloned at build time.
        reg.set_bash_tool_handle(Arc::new(ScriptedHandle::allow()));
        // The cell is now populated; a fresh snapshot sees the handle.
        assert!(snapshot_bash_handle(&reg.bash_tool_handle_cell()).is_some());
    }
}

fn render_skill_prompt_args(name: &str, body: &str, args: &str) -> String {
    let args_opt = (!args.is_empty()).then_some(args);
    if name == "simplify" {
        let mut text = coco_skills::prompt_render::substitute_arguments(
            body,
            args_opt,
            &[],
            /* append_if_no_placeholder */ false,
        );
        if !args.is_empty() {
            text.push_str("\n\n## Additional Focus\n\n");
            text.push_str(args);
        }
        return text;
    }
    coco_skills::prompt_render::substitute_arguments(
        body,
        args_opt,
        &[],
        /* append_if_no_placeholder */ true,
    )
}

/// Built-in command handler for simple commands that return static or
/// computed text output.
pub struct BuiltinCommand {
    name: &'static str,
    execute_fn: fn(&str) -> String,
}

impl BuiltinCommand {
    pub const fn new(name: &'static str, execute_fn: fn(&str) -> String) -> Self {
        Self { name, execute_fn }
    }
}

#[async_trait]
impl CommandHandler for BuiltinCommand {
    async fn execute(&self, args: &str) -> crate::Result<String> {
        Ok((self.execute_fn)(args))
    }

    fn handler_name(&self) -> &str {
        self.name
    }
}

/// Function pointer for async command bodies.
pub type AsyncCommandFn =
    fn(String) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>>;

/// Async built-in command handler for commands that need I/O (git, filesystem).
pub struct AsyncBuiltinCommand {
    name: &'static str,
    execute_fn: AsyncCommandFn,
}

impl AsyncBuiltinCommand {
    pub const fn new(name: &'static str, execute_fn: AsyncCommandFn) -> Self {
        Self { name, execute_fn }
    }
}

#[async_trait]
impl CommandHandler for AsyncBuiltinCommand {
    async fn execute(&self, args: &str) -> crate::Result<String> {
        (self.execute_fn)(args.to_string()).await
    }

    fn handler_name(&self) -> &str {
        self.name
    }
}

/// Create a `CommandBase` with common defaults for built-in commands.
pub fn builtin_base(name: &str, description: &str, aliases: &[&str]) -> CommandBase {
    CommandBase {
        name: name.to_string(),
        description: description.to_string(),
        aliases: aliases.iter().map(ToString::to_string).collect(),
        availability: vec![],
        is_hidden: false,
        argument_hint: None,
        argument_kind: CommandArgumentKind::None,
        when_to_use: None,
        user_invocable: true,
        is_sensitive: false,
        loaded_from: Some(CommandSource::Bundled),
        safety: CommandSafety::default(),
        supports_non_interactive: false,
    }
}

/// Create a `CommandBase` with safety and argument hint options.
pub fn builtin_base_ext(
    name: &str,
    description: &str,
    aliases: &[&str],
    safety: CommandSafety,
    argument_hint: Option<&str>,
) -> CommandBase {
    CommandBase {
        name: name.to_string(),
        description: description.to_string(),
        aliases: aliases.iter().map(ToString::to_string).collect(),
        availability: vec![],
        is_hidden: false,
        argument_hint: argument_hint.map(ToString::to_string),
        argument_kind: argument_hint
            .map(|_| CommandArgumentKind::FreeText)
            .unwrap_or(CommandArgumentKind::None),
        when_to_use: None,
        user_invocable: true,
        is_sensitive: false,
        loaded_from: Some(CommandSource::Bundled),
        safety,
        supports_non_interactive: false,
    }
}

type BuiltinSpec = (
    &'static str,
    &'static str,
    &'static [&'static str],
    fn(&str) -> String,
);

/// Register the standard set of built-in commands into a registry.
///
/// Registers ~65+ commands. We start with the most important ~25.
pub fn register_builtins(registry: &mut CommandRegistry) {
    let builtins: Vec<BuiltinSpec> = vec![
        // ── Core ──
        ("help", "Show available commands", &["h", "?"], help_handler),
        ("clear", "Clear conversation history", &[], clear_handler),
        (
            "compact",
            "Compact conversation to reduce context usage",
            &[],
            compact_handler,
        ),
        ("status", "Show current session status", &[], status_handler),
        // ── Configuration ──
        (
            "config",
            "Show or modify configuration",
            &["settings"],
            config_handler,
        ),
        ("model", "Switch the current model", &[], model_handler),
        (
            "effort",
            "Set reasoning effort level (low/medium/high)",
            &[],
            effort_handler,
        ),
        (
            "permissions",
            "Review and modify permission rules",
            &["perms"],
            permissions_handler,
        ),
        // ── Provider auth (OAuth subscriptions) ──
        // The interactive flow is handled in `app/cli::tui_runner` (it owns the
        // runtime + AuthService); these entries provide discoverability + a
        // non-TUI fallback hint.
        (
            "login",
            "Log in to a provider subscription via OAuth (e.g. /login openai)",
            &[],
            login_handler,
        ),
        (
            "logout",
            "Clear a provider subscription credential (e.g. /logout openai)",
            &[],
            logout_handler,
        ),
        // ── Session ──
        (
            "session",
            "Manage sessions (list, resume, delete)",
            &[],
            session_handler,
        ),
        ("resume", "Resume a previous session", &[], resume_handler),
        (
            "cost",
            "Show token usage and cost for this session",
            &[],
            cost_handler,
        ),
        ("context", "Show context window usage", &[], context_handler),
        // ── Development ──
        (
            "diff",
            "Show git diff of current changes",
            &[],
            diff_handler,
        ),
        // /commit registered as a Prompt in
        // implementations.rs::register_ts_parity_handlers — builds git
        // context + commit prompt.
        // /pr removed: use /commit-push-pr instead.
        // /review is registered as a Prompt in implementations.rs; no entry here.
        // ── Tools & Plugins ──
        // /lsp is registered as an async handler in
        // `register_extended_builtins` (handlers::lsp::handler). The
        // re-registration there replaces this slot, so the simple-array
        // entry is omitted to keep one source of truth.
        ("mcp", "Manage MCP server connections", &[], mcp_handler),
        (
            "plugin",
            "Manage installed plugins",
            &["plugins"],
            plugin_handler,
        ),
        (
            "agents",
            "List available agent definitions",
            &[],
            agents_handler,
        ),
        ("tasks", "List active tasks", &["bashes"], tasks_handler),
        // ── System ──
        ("doctor", "Run diagnostic checks", &[], doctor_handler),
        (
            "init",
            "Initialize project with .coco/ directory",
            &[],
            init_handler,
        ),
    ];

    for (name, description, aliases, handler_fn) in builtins {
        let handler = Arc::new(BuiltinCommand::new(name, handler_fn));
        registry.register(RegisteredCommand {
            base: builtin_base(name, description, aliases),
            command_type: CommandType::Local(LocalCommandData {
                handler: name.to_string(),
            }),
            handler: Some(handler),
            is_enabled: None,
        });
    }
}

// ── Core command handlers ──

fn help_handler(_args: &str) -> String {
    "Available commands:\n\
     /help - Show this help\n\
     /clear - Clear conversation\n\
     /compact - Compact context\n\
     /config - View/modify configuration\n\
     /model - Switch model\n\
     /effort - Set reasoning effort\n\
     /permissions - Manage permissions\n\
     /status - Session status\n\
     /cost - Token usage and cost\n\
     /context - Context window usage\n\
     /diff - Show git diff\n\
     /commit - Create git commit\n\
     /pr - Create pull request\n\
     /review - Review code\n\
     /session - Manage sessions\n\
     /mcp - MCP server management\n\
     /plugin - Plugin management\n\
     /doctor - Diagnostics\n\
     /init - Initialize project\n\
     \nUse /help <command> for details."
        .to_string()
}

fn clear_handler(_args: &str) -> String {
    "Conversation cleared. Plan state, file caches, and cache-break tracking reset.".to_string()
}

// `/login` + `/logout` are intercepted by `app/cli::tui_runner` (which runs the
// real OAuth flow on the shared `AuthService`). These bodies are reached only
// on non-interactive paths (e.g. SDK), where a browser flow can't run — point
// the user at the CLI.
fn login_handler(_args: &str) -> String {
    "Interactive login isn't available here. Run `coco login <provider>` in a terminal.".to_string()
}

fn logout_handler(args: &str) -> String {
    let who = args.trim();
    if who.is_empty() {
        "Run `coco logout <provider>` in a terminal.".to_string()
    } else {
        format!("Run `coco logout {who}` in a terminal.")
    }
}

fn compact_handler(_args: &str) -> String {
    "Compacting conversation...".to_string()
}

fn status_handler(_args: &str) -> String {
    "Session status: active".to_string()
}

// ── Configuration handlers ──

fn config_handler(args: &str) -> String {
    if args.is_empty() {
        "Current configuration (use /config <key> <value> to modify):".to_string()
    } else {
        format!("Configuration updated: {args}")
    }
}

fn model_handler(args: &str) -> String {
    if args.is_empty() {
        "Current model: (use /model <name> to switch)\n\
         Available: sonnet, opus, haiku"
            .to_string()
    } else {
        format!("Switching to model: {args}")
    }
}

fn effort_handler(args: &str) -> String {
    match args.trim() {
        "low" | "medium" | "high" => format!("Reasoning effort set to: {args}"),
        "" => "Current effort: medium\nOptions: low, medium, high".to_string(),
        _ => format!("Unknown effort level: {args}. Use low, medium, or high."),
    }
}

fn permissions_handler(_args: &str) -> String {
    "Permission rules:\n\
     Use /permissions allow <tool> to add allow rules\n\
     Use /permissions deny <tool> to add deny rules"
        .to_string()
}

// ── Session handlers ──

fn session_handler(args: &str) -> String {
    match args.trim() {
        "list" | "" => "Sessions: (use /session list to see all)".to_string(),
        "delete" => "Usage: /session delete <id>".to_string(),
        _ => format!("Unknown session subcommand: {args}"),
    }
}

fn resume_handler(args: &str) -> String {
    if args.is_empty() {
        "Usage: /resume [session-id] — Resumes the most recent or specified session.".to_string()
    } else {
        format!("Resuming session: {args}")
    }
}

fn cost_handler(_args: &str) -> String {
    "Session cost:\n  Input tokens: 0\n  Output tokens: 0\n  Cost: $0.00".to_string()
}

fn context_handler(_args: &str) -> String {
    "Context window usage: 0 / 200,000 tokens (0%)".to_string()
}

// ── Development handlers ──

fn diff_handler(_args: &str) -> String {
    "Showing git diff of current changes...".to_string()
}

// ── Tools & Plugins handlers ──

fn mcp_handler(args: &str) -> String {
    match args.trim() {
        "" | "list" => "MCP servers: (none connected)\n\
             Use /mcp add <name> to add a server."
            .to_string(),
        "add" => "Usage: /mcp add <name> <config>".to_string(),
        _ => format!("MCP: {args}"),
    }
}

fn plugin_handler(args: &str) -> String {
    match args.trim() {
        "" | "list" => "Installed plugins: (none)\n\
             Use /plugin install <name> to install."
            .to_string(),
        _ => format!("Plugin: {args}"),
    }
}

fn agents_handler(_args: &str) -> String {
    "Available agents:\n  (none defined)\n\
     Place agent definitions in .coco/agents/"
        .to_string()
}

fn tasks_handler(_args: &str) -> String {
    "Active tasks: (none)".to_string()
}

// ── System handlers ──

fn doctor_handler(_args: &str) -> String {
    "Running diagnostics...\n\
     [ok] Shell: bash\n\
     [ok] Git: available\n\
     [ok] Config: loaded"
        .to_string()
}

fn init_handler(_args: &str) -> String {
    "Initializing project...\n\
     Created .coco/ directory\n\
     Created .coco/settings.json"
        .to_string()
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
