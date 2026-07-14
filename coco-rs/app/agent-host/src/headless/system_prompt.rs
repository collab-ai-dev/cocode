use super::*;

/// Fallback base instructions used when a resolved `ModelInfo`
/// declares no `base_instructions` (e.g. Claude built-ins and any
/// user-added non-builtin model in `config home/providers.json` /
/// `models.json` that doesn't set `base_instructions[_file]`). Routed
/// through `coco_config::DEFAULT_BASE_INSTRUCTIONS` so the on-disk
/// `instructions/default_prompt.md` is the single source of truth.
pub const DEFAULT_SYSTEM_PROMPT_IDENTITY: &str = coco_config::DEFAULT_BASE_INSTRUCTIONS;

// ─── Output style manager ────────────────────────────────────────────

/// Build a [`coco_output_styles::OutputStyleManager`] from settings,
/// the standard on-disk dirs ([`crate::paths::user_output_style_dir`],
/// [`crate::paths::project_output_style_dirs`],
/// [`crate::paths::managed_output_style_dir`]), and the supplied
/// plugin sources.
/// Headless and AppServer paths share this helper so a future addition (e.g.,
/// project-tree ancestor walk) lands in one place. `plugin_sources` are the
/// plugin-contributed output-style directories (see
/// [`coco_app_runtime::ProjectServices::output_style_sources`]).
pub fn build_output_style_manager(
    runtime_config: &coco_config::RuntimeConfig,
    cwd: &Path,
    plugin_sources: &[coco_output_styles::PluginOutputStyleSource],
) -> coco_output_styles::OutputStyleManager {
    coco_output_styles::OutputStyleManager::builder()
        .settings_name(runtime_config.settings.merged.output_style.clone())
        .user_dir(Some(crate::paths::user_output_style_dir()))
        .project_dirs(crate::paths::project_output_style_dirs(cwd))
        .managed_dir(Some(crate::paths::managed_output_style_dir()))
        .plugins(plugin_sources.to_vec())
        .build()
}

// ─── System prompt assembly ──────────────────────────────────────────

/// Convert a resolved [`OutputStyleConfig`] into the borrowed view the
/// `coco-context` prompt builder accepts.
fn output_style_section(
    style: &coco_output_styles::OutputStyleConfig,
) -> coco_context::prompt::OutputStyleSection<'_> {
    coco_context::prompt::OutputStyleSection {
        name: &style.name,
        prompt: &style.prompt,
        // Built-in styles set keep_coding_instructions: Some (true);
        // unset custom/plugin styles default to false, matching the strict
        // `keepCodingInstructions === true` gate.
        keep_coding_instructions: style.keep_coding_instructions.unwrap_or(false),
    }
}

/// Build the system prompt with environment context and CLAUDE.md content.
pub fn build_system_prompt(
    cwd: &Path,
    model_id: &str,
    base_instructions: Option<&str>,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
    additional_working_directories: &[String],
    include_git_status: bool,
) -> String {
    let claude_files = coco_context::discover_memory_files(cwd);
    let env_info = coco_context::get_environment_info(cwd, model_id, include_git_status);
    let default_identity;
    let identity = if let Some(base_instructions) = base_instructions {
        base_instructions
    } else {
        default_identity = coco_config::default_base_instructions();
        &default_identity
    };
    let section = output_style.map(output_style_section);
    coco_context::build_system_prompt(
        identity,
        &claude_files,
        &env_info,
        None,
        None,
        None,
        section,
        additional_working_directories,
    )
    .full_text()
}

/// Resolve model-specific instructions from runtime config, then build
/// the prompt. Shared by headless, AppServer, and TUI bootstraps.
pub fn build_system_prompt_for_model(
    cwd: &Path,
    runtime_config: &coco_config::RuntimeConfig,
    provider: &str,
    model_id: &str,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
    additional_working_directories: &[String],
) -> String {
    let resolved = runtime_config.model_registry.resolve(provider, model_id);
    let base_instructions = resolved
        .as_ref()
        .and_then(|model| model.info.base_instructions.as_deref());
    // Point the "Break down and manage your work with the <X> tool" nudge at
    // whichever task tool is actually live. The two are mutually exclusive:
    // TaskV2 on → TaskCreate, off → TodoWrite (see `task_tools.rs::is_enabled`).
    // The default prompt names TaskCreate, so only V1 needs a rewrite. Mirrors
    // `getUsingYourToolsSection`'s `taskToolName = [TaskCreate, TodoWrite]
    // .find (enabled)`; `replace` is a no-op for prompts without the bullet.
    let base_instructions: Option<String> = base_instructions.map(|base| {
        if runtime_config.features.enabled(coco_types::Feature::TaskV2) {
            base.to_string()
        } else {
            base.replace(
                &format!(
                    "with the {} tool",
                    coco_types::ToolName::TaskCreate.as_str()
                ),
                &format!("with the {} tool", coco_types::ToolName::TodoWrite.as_str()),
            )
        }
    });
    // Suppress the git-status block under COCO_REMOTE or a disabled
    // `include_git_instructions` setting (COCO_DISABLE_GIT_INSTRUCTIONS
    // overrides the setting either way).
    let env = coco_config::EnvSnapshot::from_current_process();
    let include_git_status = !env.is_truthy(coco_config::EnvKey::CocoRemote)
        && coco_config::gitsettings::should_include_git_instructions(
            &runtime_config.settings.merged,
            &env,
        );
    build_system_prompt(
        cwd,
        model_id,
        base_instructions.as_deref(),
        output_style,
        additional_working_directories,
        include_git_status,
    )
}

/// Compose the session's system prompt, honoring `--system-prompt`
/// (full override), `--append-system-prompt` (text appended after the
/// default), and `--append-system-prompt-file` (file contents appended).
pub(crate) fn compose_system_prompt(
    cli: &AgentHostOptions,
    cwd: &Path,
    runtime_config: &coco_config::RuntimeConfig,
    provider: &str,
    model_id: &str,
    output_style: Option<&coco_output_styles::OutputStyleConfig>,
) -> Result<String> {
    // 1. Base layer: `--system-prompt` wholly replaces the default
    // identity + CLAUDE.md discovery. Otherwise build the default.
    let additional_dirs = resolve_additional_dirs_display(cli, cwd);
    let mut prompt = if let Some(custom) = cli.system_prompt.as_deref() {
        custom.to_string()
    } else {
        build_system_prompt_for_model(
            cwd,
            runtime_config,
            provider,
            model_id,
            output_style,
            &additional_dirs,
        )
    };
    // 2. Append from `--append-system-prompt` (verbatim).
    if let Some(append) = cli.append_system_prompt.as_deref() {
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push_str(append);
    }
    // 3. Append from `--append-system-prompt-file` (read once, fail
    // fast if the file's missing rather than silently dropping).
    if let Some(path) = cli.append_system_prompt_file.as_deref() {
        let body = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("--append-system-prompt-file {path:?}: {e}"))?;
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push_str(&body);
    }
    Ok(prompt)
}
