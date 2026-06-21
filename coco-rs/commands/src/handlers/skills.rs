//! `/skills` — open the editable 4-state override dialog or run the
//! text subcommand variants.
//!
//! coco mirrors that no-arg dialog: the invocation returns a
//! [`crate::CommandResult::OpenDialog`] carrying a fully-built
//! [`coco_types::SkillsDialogPayload`] with every row pre-populated:
//! `description`, `frontmatter_bytes`, `current_local`, `baseline`,
//! and `lock` — the TUI consumer renders without recomputing.
//!
//! Sub-commands (`list` / `show <name>` / `paths`) are a coco-only extension on
//! top of the picker — text output so SDK / headless / scripted callers get
//! a flat enumeration they can parse.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use coco_config::SkillOverrideTiers;
use coco_skills::SkillDefinition;
use coco_skills::SkillManager;
use coco_skills::SkillSource;
use coco_skills::estimate_skill_frontmatter_bytes;
use coco_skills::get_managed_skills_path;
use coco_skills::resolve_skill_baseline;
use coco_skills::resolve_skill_override_lock;
use coco_types::SkillOverrideState;
use coco_types::SkillsDialogEntry;
use coco_types::SkillsDialogPayload;
use coco_types::SkillsDialogSource;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

/// Placeholder bytes-per-token ratio shipped by the handler before
/// the CLI bridge knows which model is active. The real value is
/// stamped in by `apply_model_token_density` in `tui_runner.rs` once
/// the live `QueryEngineConfig.model_id` is in scope. `4` is the
/// Claude-family default — see
/// [`coco_model_card::bytes_per_token_for_model`].
const PLACEHOLDER_BYTES_PER_TOKEN: i64 = 4;

/// `CommandHandler` impl for `/skills`. No args → open the TUI
/// dialog; `list` / `show` / `paths` → reuse the text path.
pub struct SkillsHandler;

#[async_trait]
impl CommandHandler for SkillsHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let trimmed = args.trim().to_string();
        let cwd = std::env::current_dir().unwrap_or_default();
        let config_home = coco_config::global_config::config_home();

        // Discovery is sync (`std::fs`) — keep the TUI event loop
        // unblocked.
        //
        // PR3 limitation: the payload is built without a
        // `SkillOverrideTiers` because the command handler trait
        // does not carry `RuntimeConfig`. The caller (TUI overlay
        // owner) merges the live tiers into the payload before
        // rendering. See `app/tui/src/state/surface_payloads.rs`
        // for the `SkillsDialogPayload::from_wire` + `with_tiers`
        // composition.
        tokio::task::spawn_blocking(move || -> crate::Result<CommandResult> {
            if trimmed.is_empty() {
                let payload =
                    build_dialog_payload(&config_home, &cwd, &SkillOverrideTiers::default());
                Ok(CommandResult::OpenDialog(DialogSpec::SkillsList {
                    payload,
                }))
            } else {
                Ok(CommandResult::Text(render(&trimmed, &config_home, &cwd)?))
            }
        })
        .await
        .map_err(|e| crate::CommandsError::generic(format!("skills handler join error: {e}")))?
    }

    fn handler_name(&self) -> &str {
        "skills"
    }
}

/// Build the dialog payload from the freshly-discovered skill
/// catalog, populating every per-row override field so the dialog
/// can render + save without round-tripping to the handler.
///
/// **Bundled skills appear in the dialog** — shown with the `built-in`
/// source label and not locked, so the user can disable a noisy bundled
/// skill via `/skills`.
///
/// **Conditional (`paths`-gated) skills appear too** — they sit in
/// `SkillManager.disk_conditional` until activated by a matching
/// file touch, but the dialog uses
/// [`SkillManager::all_including_conditional`] so a user can
/// override them ahead of activation.
pub fn build_dialog_payload(
    config_home: &Path,
    cwd: &Path,
    tiers: &SkillOverrideTiers,
) -> SkillsDialogPayload {
    let manager = build_manager(config_home, cwd);
    let skills = manager.all_including_conditional();

    let entries: Vec<SkillsDialogEntry> = skills
        .iter()
        .filter(|s| !s.disabled)
        .map(|s| {
            let source = source_to_dialog(&s.source);
            let baseline = resolve_skill_baseline(&s.name, tiers);
            let current_local = tiers.local.get(&s.name).copied();
            let lock = resolve_skill_override_lock(s, tiers);
            SkillsDialogEntry {
                name: s.name.clone(),
                source,
                description: s.description.clone(),
                plugin_name: plugin_name_for(s),
                frontmatter_bytes: estimate_skill_frontmatter_bytes(s) as i64,
                current_local,
                baseline,
                lock,
            }
        })
        .collect();

    SkillsDialogPayload {
        entries,
        bytes_per_token: PLACEHOLDER_BYTES_PER_TOKEN,
    }
}

/// Map a [`SkillSource`] to the dialog's source discriminant.
/// **Bundled and Managed both surface** — the former under
/// `BuiltIn`, the latter under `Policy`. There are no longer any
/// dialog-excluded sources.
fn source_to_dialog(source: &SkillSource) -> SkillsDialogSource {
    match source {
        SkillSource::Bundled => SkillsDialogSource::BuiltIn,
        SkillSource::Project { .. } => SkillsDialogSource::Project,
        SkillSource::User { .. } => SkillsDialogSource::User,
        SkillSource::Managed { .. } => SkillsDialogSource::Policy,
        SkillSource::Plugin { .. } => SkillsDialogSource::Plugin,
        SkillSource::Mcp { .. } => SkillsDialogSource::Mcp,
    }
}

fn plugin_name_for(s: &SkillDefinition) -> Option<String> {
    match &s.source {
        SkillSource::Plugin { plugin_name } => Some(plugin_name.clone()),
        _ => None,
    }
}

/// Pin one MCP-server name → `SkillsDialogSource::Mcp` for any
/// caller that still wants the per-source MCP server list (the
/// dialog itself no longer groups, but external SDK clients that
/// build their own view can scan `entries` for `Mcp` sources).
#[allow(dead_code)]
fn mcp_servers(skills: &[Arc<SkillDefinition>]) -> Vec<String> {
    let mut servers: Vec<String> = skills
        .iter()
        .filter_map(|s| match &s.source {
            SkillSource::Mcp { server_name } => Some(server_name.clone()),
            _ => None,
        })
        .collect();
    servers.sort();
    servers.dedup();
    servers
}

#[allow(dead_code)]
fn managed_skills_path_display() -> String {
    get_managed_skills_path().display().to_string()
}

#[allow(dead_code)]
fn project_skills_paths(cwd: &Path) -> Vec<String> {
    coco_skills::watcher::session_reload_scopes(Path::new(""), cwd)
        .into_iter()
        .filter_map(|scope| match scope {
            coco_skills::watcher::SkillReloadScope::Project(path) => {
                Some(path.display().to_string())
            }
            coco_skills::watcher::SkillReloadScope::Managed(_)
            | coco_skills::watcher::SkillReloadScope::User(_) => None,
        })
        .collect()
}

/// Drop-in helper that lets a downstream consumer (TUI overlay
/// builder) re-resolve overrides against the live `RuntimeConfig`
/// tiers when the handler couldn't (the handler runs without a
/// `RuntimeConfig` in scope today).
pub fn enrich_payload_with_tiers(
    payload: &mut SkillsDialogPayload,
    tiers: &SkillOverrideTiers,
    skills: &SkillManager,
) {
    for entry in payload.entries.iter_mut() {
        let Some(skill) = skills.get(&entry.name) else {
            continue;
        };
        entry.baseline = resolve_skill_baseline(&entry.name, tiers);
        entry.current_local = tiers.local.get(&entry.name).copied();
        entry.lock = resolve_skill_override_lock(&skill, tiers);
    }
}

/// Synthesize a single SkillOverrideEdit for the diff-against-baseline
/// save algorithm. Used by tests + the dialog wiring to keep the
/// override-diff logic in one place.
#[allow(dead_code)]
pub fn compute_save_value(
    baseline: SkillOverrideState,
    pending: SkillOverrideState,
) -> Option<SkillOverrideState> {
    if pending == baseline {
        None
    } else {
        Some(pending)
    }
}

fn render(args: &str, config_home: &Path, cwd: &Path) -> crate::Result<String> {
    let manager = build_manager(config_home, cwd);

    let (cmd, rest) = match args.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (args, ""),
    };

    Ok(match cmd {
        "" | "list" => render_list(&manager),
        "show" => render_show(&manager, rest),
        "paths" => render_paths(config_home, cwd),
        // `/skills <name>` is a UX shorthand for `/skills show <name>`.
        // The skills menu is read-only; skills are invoked by typing `/<name>`
        // directly. We accept the shorthand so the flat-text path matches
        // `/agents <name>` and saves a keystroke; users still invoke a
        // skill via `/<name>`.
        other if manager.get(other).is_some() => render_show(&manager, other),
        other => format!(
            "Unknown /skills subcommand: {other}\n\nUsage: /skills [list|show <name>|paths]\nTo run a skill, type /<skill-name>."
        ),
    })
}

/// Build a `SkillManager` with **source-correct tagging** so both the
/// dialog (which groups by source) and the text `list` output (which
/// labels each row by source) get the right `[user]` / `[project]` /
/// `[managed]` attribution.
///
/// Built fresh per invocation so newly-added skills surface without a
/// session restart — the engine's live registry still loads only at
/// startup, but `/skills` reflects current disk truth.
///
/// Builds the same catalog the session runtime uses (bundled + managed /
/// user / project scopes), so the `/skills` dialog and `/context` never
/// disagree. See [`coco_skills::build_session_skill_manager`].
fn build_manager(config_home: &Path, cwd: &Path) -> SkillManager {
    // The `CommandHandler` trait does not carry `RuntimeConfig`, so the
    // `/skills` dialog cannot resolve `--setting-sources` / policy gates here
    // (same PR3 limitation as the `SkillOverrideTiers`). All scopes load; the
    // engine's live registry — built via `build_session_skill_manager` with
    // resolved gates — is the policy-enforcing catalog.
    coco_skills::build_session_skill_manager(
        config_home,
        cwd,
        &coco_skills::SkillLoadGates::all_enabled(),
    )
}

fn render_list(manager: &SkillManager) -> String {
    let mut skills = manager.all();
    if skills.is_empty() {
        return "No skills found.\n\
                Place SKILL.md directories in ~/.coco/skills (user) or \
                <project>/.coco/skills (project)."
            .to_string();
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = format!("{} skill(s) loaded:\n\n", skills.len());
    for s in &skills {
        let source = source_label(&s.source);
        out.push_str(&format!("  /{}  [{source}]\n", s.name));
        let desc = s.description.lines().next().unwrap_or(&s.description);
        out.push_str(&format!("    {desc}\n"));
        if !s.aliases.is_empty() {
            out.push_str(&format!("    aliases: {}\n", s.aliases.join(", ")));
        }
    }
    // Invocation hint — the registry registers each skill as a command
    // (commands::register_skills_as_commands), so `/<name>` runs it.
    out.push_str(
        "\nTo run a skill: type /<skill-name>.\n\
         Details: /skills show <name>",
    );
    out
}

fn render_show(manager: &SkillManager, name: &str) -> String {
    if name.is_empty() {
        return "Usage: /skills show <name>".to_string();
    }
    let Some(s) = manager.get(name) else {
        return format!("No skill named: {name}");
    };

    let mut out = format!("# {}\n\n", s.name);
    out.push_str(&format!("Source:        {}\n", source_label(&s.source)));
    out.push_str(&format!("Description:   {}\n", s.description));
    if let Some(model) = &s.model {
        out.push_str(&format!("Model:         {model}\n"));
    }
    if let Some(when) = &s.when_to_use {
        out.push_str(&format!("When to use:   {when}\n"));
    }
    if let Some(hint) = &s.argument_hint {
        out.push_str(&format!("Args:          {hint}\n"));
    }
    if let Some(tools) = &s.allowed_tools
        && !tools.is_empty()
    {
        out.push_str(&format!("Tools:         {}\n", tools.join(", ")));
    }
    if !s.aliases.is_empty() {
        out.push_str(&format!("Aliases:       {}\n", s.aliases.join(", ")));
    }

    let preview = s.prompt.lines().take(10).collect::<Vec<_>>().join("\n");
    if !preview.is_empty() {
        out.push_str("\nPrompt preview:\n");
        out.push_str(&preview);
        if s.prompt.lines().count() > 10 {
            out.push_str("\n...");
        }
    }
    out
}

fn render_paths(config_home: &Path, cwd: &Path) -> String {
    let mut out = String::from("Skill search paths (earlier sources have priority):\n\n");
    out.push_str("  bundled  (compiled-in catalog)\n");
    out.push_str(&format!(
        "  user     {}\n",
        config_home.join("skills").display()
    ));
    for path in project_skills_paths(cwd) {
        out.push_str(&format!("  project  {path}\n"));
    }
    out
}

fn source_label(source: &SkillSource) -> String {
    match source {
        SkillSource::Bundled => "bundled".to_string(),
        SkillSource::User { path } => format!("user · {}", trim_path(path)),
        SkillSource::Project { path } => format!("project · {}", trim_path(path)),
        SkillSource::Plugin { plugin_name } => format!("plugin · {plugin_name}"),
        SkillSource::Managed { path } => format!("managed · {}", trim_path(path)),
        SkillSource::Mcp { server_name } => format!("mcp · {server_name}"),
    }
}

fn trim_path(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
#[path = "skills.test.rs"]
mod tests;
