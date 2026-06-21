//! System prompt building.
//!
//! Assembles the prompt as an ordered list of `SystemPromptBlock`s
//! with explicit cache breakpoints; the final cache-prefix placement
//! keeps static identity + style + project instructions cached
//! together, with environment + memory + custom-append placed after.

use serde::Deserialize;
use serde::Serialize;

/// Condensed notes block used by subagents only.
///
/// **SUBAGENT-ONLY** by design. The main agent gets richer per-section
/// rules (`getSimpleToneAndStyleSection`, `getActionsSection`,
/// `getOutputEfficiencySection`) instead of this 4-bullet concentrate.
/// Subagents skip those sections and receive these condensed bullets
/// via `enhanceSystemPromptWithEnvDetails`.
///
/// Exposed `pub` so the subagent spawn path
/// (`coordinator::spawn.rs::build_fresh_prompt`) can pass it through
/// as the `notes_after_env` slot of [`build_system_prompt`]. The main
/// agent path (`headless::build_system_prompt_for_model`) passes
/// `None` for that slot and therefore does NOT receive this block.
///
/// Provenance metadata for the data file lives in
/// `agent_notes.SOURCE.md` (kept out of the `include_str!`'d body so
/// it doesn't pollute the model's view).
pub const AGENT_NOTES: &str = include_str!("agent_notes.md");

/// Default identity used when the caller doesn't supply one.
/// Subagent-only fallback for spawns whose `AgentDefinition` has an
/// empty `system_prompt` body (edge case: missing catalog entry,
/// malformed `.md`). The main agent uses
/// `DEFAULT_SYSTEM_PROMPT_IDENTITY` from `app/cli/src/headless.rs`,
/// which is the full base instructions ("You are an interactive agent …").
pub const DEFAULT_AGENT_IDENTITY: &str = "You are Coco, a CLI coding assistant. Given the user's message, you should use the tools available to complete the task. Complete the task fully—don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.";

/// A compiled system prompt with cache breakpoints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemPrompt {
    pub blocks: Vec<SystemPromptBlock>,
}

/// A block within the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemPromptBlock {
    Text { content: String },
    CacheBreakpoint,
}

impl SystemPrompt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a text section.
    pub fn add_text(&mut self, text: impl Into<String>) {
        self.blocks.push(SystemPromptBlock::Text {
            content: text.into(),
        });
    }

    /// Add a cache breakpoint.
    pub fn add_cache_breakpoint(&mut self) {
        self.blocks.push(SystemPromptBlock::CacheBreakpoint);
    }

    /// Get the full prompt text (without cache breakpoints).
    pub fn full_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                SystemPromptBlock::Text { content } => Some(content.as_str()),
                SystemPromptBlock::CacheBreakpoint => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Estimated token count (rough: 1 token ≈ 4 chars).
    pub fn estimated_tokens(&self) -> i64 {
        (self.full_text().len() as i64) / 4
    }
}

/// Borrowed view of an active output style for the prompt builder.
///
/// Defined locally so `coco-context` does not depend on
/// `coco-output-styles` (which would break the
/// `core/` → `root/` layering rule). The CLI converts an
/// `OutputStyleConfig` to this view at the boundary.
///
/// Mirrors `getOutputStyleSection` + `getSimpleIntroSection` (the
/// `outputStyleConfig !== null` branch alters intro framing) + the
/// `keepCodingInstructions` gate that suppresses the standard "Doing
/// tasks" block when `false`.
#[derive(Debug, Clone, Copy)]
pub struct OutputStyleSection<'a> {
    /// Display name (e.g., `Explanatory`, `alpha:concise`).
    pub name: &'a str,
    /// Full prompt body.
    pub prompt: &'a str,
    /// When `true`, the standard coding instructions stay on top of
    /// the style. When `false`, the style replaces them. Keeps
    /// those instructions for default style or explicit
    /// `keepCodingInstructions: true`.
    pub keep_coding_instructions: bool,
}

/// Build a complete system prompt from all context sources.
///
/// Assembles identity, output style, project instructions, environment,
/// skills, memory, and custom append. The `output_style` parameter,
/// when present, is injected immediately after the identity block (and
/// before the cache breakpoint) so the cached prefix covers identity +
/// style + project instructions.
///
/// Note: intro phrasing (`with software engineering tasks` vs
/// `according to your "Output Style" below`) and conditional "Doing
/// tasks" section emission based on `keepCodingInstructions` are not
/// applied here — callers build the identity string with awareness of
/// output-style presence (e.g., the binary embedded prompt swap). The
/// `keep_coding_instructions` flag is surfaced on `OutputStyleSection`
/// for future use; it does not short-circuit the current static
/// identity.
/// `notes_after_env` is appended **immediately after** the env block
/// — before skill listing, memory, or any other dynamic section. The
/// subagent path uses this slot for [`AGENT_NOTES`] so the model sees
/// behavior rules *before* memory content. The notes are bundled
/// directly with the env block (not after memory).
#[allow(clippy::too_many_arguments)] // each arg is a distinct prompt section; bundling into a
// params struct would obscure the assembly order — and every callsite passes positional `None`s for
// inactive slots, which read clearly as "no skill listing / no memory" rather than
// `BuildPromptParams { skill_listing: None, memory_section: None, .. }`.
pub fn build_system_prompt(
    identity: &str,
    claude_md_files: &[crate::MemoryFile],
    environment: &crate::EnvironmentInfo,
    skill_listing: Option<&str>,
    memory_section: Option<&str>,
    notes_after_env: Option<&str>,
    output_style: Option<OutputStyleSection<'_>>,
    additional_working_directories: &[String],
) -> SystemPrompt {
    let mut prompt = SystemPrompt::new();

    // Identity block (who the assistant is)
    prompt.add_text(identity);

    // Output style — placed immediately after identity so the cached
    // static prefix covers it. Rendered as
    // `# Output Style: <name>\n<prompt>`.
    if let Some(style) = output_style {
        prompt.add_text(format!(
            "\n# Output Style: {}\n{}",
            style.name, style.prompt
        ));
    }

    prompt.add_cache_breakpoint();

    // CLAUDE.md files (project instructions)
    if !claude_md_files.is_empty() {
        let mut claude_section = String::from("# Project Instructions\n\n");
        for file in claude_md_files {
            claude_section.push_str(&format!("## {}\n{}\n\n", file.path.display(), file.content));
        }
        prompt.add_text(claude_section);
        prompt.add_cache_breakpoint();
    }

    // Environment block. The `<env>` XML wrapping is the structural
    // delimiter; keep it for parsing consistency.
    prompt.add_text(render_env_block(
        environment,
        additional_working_directories,
    ));

    // Git status snapshot — appended as `gitStatus: <value>`. Rendered
    // immediately after `<env>` so it shares the cached session-start
    // prefix (the status is a snapshot taken at session start, stable
    // for the conversation). Present only in git repos —
    // `get_environment_info` leaves `git_status` `None` otherwise.
    if let Some(git) = &environment.git_status {
        prompt.add_text(render_git_status_block(git));
    }

    // `notes_after_env` — subagent path bundles notes immediately
    // after the env block (BEFORE memory). Placing it here keeps that
    // ordering intact. Main agent passes `None` because it has richer
    // per-section rules instead of these 4 condensed bullets.
    if let Some(notes) = notes_after_env
        && !notes.is_empty()
    {
        prompt.add_text(format!("\n{notes}"));
    }

    // Skill listing (available /commands)
    if let Some(skills) = skill_listing
        && !skills.is_empty()
    {
        prompt.add_text(format!("\n# Available Skills\n{skills}"));
    }

    // Auto-memory block: type taxonomy + how-to-save + MEMORY.md.
    // Cache-broken so MEMORY.md edits don't invalidate the identity
    // / CLAUDE.md prefix above it.
    if let Some(memory) = memory_section
        && !memory.is_empty()
    {
        prompt.add_cache_breakpoint();
        prompt.add_text(memory);
    }

    prompt
}

/// Render the `<env>...</env>` block + model line + knowledge cutoff
/// line. Pure function — exposed `pub(crate)` for testing.
fn render_env_block(env: &crate::EnvironmentInfo, additional_dirs: &[String]) -> String {
    let mut s = String::new();
    s.push_str("Here is useful information about the environment you are running in:\n");
    s.push_str("<env>\n");
    s.push_str(&format!("Working directory: {}\n", env.cwd));
    s.push_str(&format!(
        "Is directory a git repo: {}\n",
        if env.is_git_repo { "Yes" } else { "No" }
    ));
    if !additional_dirs.is_empty() {
        s.push_str(&format!(
            "Additional working directories: {}\n",
            additional_dirs.join(", ")
        ));
    }
    s.push_str(&format!("Platform: {}\n", env.platform.ts_name()));
    s.push_str(&render_shell_line(env.shell));
    s.push_str(&format!("OS Version: {}\n", env.os_version));
    s.push_str("</env>\n");
    if !env.model.is_empty() {
        s.push_str(&format!("You are powered by the model {}.\n", env.model));
    }
    if !env.knowledge_cutoff.is_empty() {
        s.push_str(&format!(
            "Assistant knowledge cutoff is {}.\n",
            env.knowledge_cutoff
        ));
    }
    s
}

/// `git status --short` is truncated past this character count.
const MAX_STATUS_CHARS: usize = 2000;

/// Render the start-of-conversation git status block. The branch /
/// main-branch / user / dirty-file / recent-commits snapshot gives the
/// model start-of-session repo awareness for commit / PR / review work.
fn render_git_status_block(git: &crate::GitStatus) -> String {
    let truncated_status = if git.status.chars().count() > MAX_STATUS_CHARS {
        let head: String = git.status.chars().take(MAX_STATUS_CHARS).collect();
        format!(
            "{head}\n... (truncated because it exceeds 2k characters. If you need more information, run \"git status\" using BashTool)"
        )
    } else {
        git.status.clone()
    };
    let status_body = if truncated_status.is_empty() {
        "(clean)"
    } else {
        truncated_status.as_str()
    };

    let mut parts = vec![
        "This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.".to_string(),
        format!("Current branch: {}", git.branch),
        format!(
            "Main branch (you will usually use this for PRs): {}",
            git.main_branch.as_deref().unwrap_or_default()
        ),
    ];
    if let Some(user) = &git.user
        && !user.is_empty()
    {
        parts.push(format!("Git user: {user}"));
    }
    parts.push(format!("Status:\n{status_body}"));
    parts.push(format!("Recent commits:\n{}", git.recent_commits));

    format!("gitStatus: {}", parts.join("\n\n"))
}

/// Shell info line — includes Windows-only Unix-syntax hint.
fn render_shell_line(shell: crate::ShellKind) -> String {
    let name = shell.ts_name();
    if matches!(crate::Platform::current(), crate::Platform::Windows) {
        format!(
            "Shell: {name} (use Unix shell syntax, not Windows — e.g., /dev/null not NUL, forward slashes in paths)\n"
        )
    } else {
        format!("Shell: {name}\n")
    }
}

/// Build a minimal system prompt (for testing or non-interactive mode).
pub fn build_minimal_prompt(cwd: &std::path::Path) -> SystemPrompt {
    let env = crate::get_environment_info(cwd, "", /*include_git_status*/ true);
    let claude_files = crate::discover_memory_files(cwd);

    build_system_prompt(
        "You are an AI coding assistant. Be concise and helpful.",
        &claude_files,
        &env,
        None,
        None,
        None,
        None,
        &[],
    )
}

#[cfg(test)]
#[path = "prompt.test.rs"]
mod tests;
