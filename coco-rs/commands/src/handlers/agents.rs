//! `/agents` — open the agents overlay, or run a text sub-command.
//!
//! coco-rs exposes text sub-commands (`list` / `show <name>` / `paths` /
//! `validate` / `reload`) as a coco-only extension for SDK / headless /
//! scripted callers that can't open a TUI overlay.
//!
//! Backed by the `coco-subagent` catalog: built-ins from
//! [`coco_subagent::BuiltinAgentCatalog::interactive`] plus markdown agents
//! discovered under `~/.coco/agents` (user) and `<cwd>/.coco/agents`
//! (project). Source precedence is applied by the store; we only render
//! the snapshot here.
//!
//! Two execution paths:
//!   - **Interactive (`/agents` with no args)** — returns
//!     [`crate::CommandResult::OpenDialog`] carrying an
//!     [`coco_types::AgentsDialogPayload`] so the TUI mounts the 2-tab
//!     overlay. The CLI bridge (`tui_runner.rs`) enriches the payload
//!     with running counts at the moment the overlay opens.
//!   - **Sub-commands** (`list` / `show <name>` / `paths` / `validate`
//!     / `reload`) — fall back to text output so SDK + headless +
//!     scripted callers get a flat enumeration they can parse.

use std::path::Path;

use async_trait::async_trait;
use coco_subagent::AgentDefinitionStore;
use coco_subagent::BuiltinAgentCatalog;
use coco_subagent::definition_store::AgentSearchPaths;
use coco_types::AgentsDialogEntry;
use coco_types::AgentsDialogPayload;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

/// `CommandHandler` impl for `/agents`. No args → open the TUI
/// dialog; sub-commands → reuse the existing text renderers.
pub struct AgentsHandler;

#[async_trait]
impl CommandHandler for AgentsHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let trimmed = args.trim().to_string();
        let cwd = std::env::current_dir().unwrap_or_default();
        let config_home = coco_config::global_config::config_home();
        let paths = standard_search_paths(&config_home, &cwd);

        tokio::task::spawn_blocking(move || -> crate::Result<CommandResult> {
            if trimmed.is_empty() {
                let payload = build_dialog_payload(paths);
                Ok(CommandResult::OpenDialog(DialogSpec::AgentsList {
                    payload,
                }))
            } else {
                Ok(CommandResult::Text(render(&trimmed, paths)?))
            }
        })
        .await
        .map_err(|e| crate::CommandsError::generic(format!("agents handler join error: {e}")))?
    }

    fn handler_name(&self) -> &str {
        "agents"
    }
}

/// Build the Library payload for the `/agents` overlay.
///
/// `is_overridden` is set per row by walking
/// `AgentCatalogSnapshot::all()` for entries whose `agent_type`
/// matches but whose source priority is lower than the active one.
fn build_dialog_payload(paths: AgentSearchPaths) -> AgentsDialogPayload {
    let mut store = AgentDefinitionStore::new(BuiltinAgentCatalog::interactive(), paths);
    store.load();
    let snapshot = store.snapshot();

    // Map active name → its winning source so we can flag overridden
    // entries when walking `all()`.
    let active_source: std::collections::BTreeMap<String, coco_types::AgentSource> = snapshot
        .active()
        .map(|d| (d.name.clone(), d.source))
        .collect();

    let entries: Vec<AgentsDialogEntry> = snapshot
        .all()
        .iter()
        .map(|loaded| {
            let def = &loaded.definition;
            let is_overridden = active_source
                .get(&def.name)
                .map(|winning| *winning != def.source)
                .unwrap_or(false);
            AgentsDialogEntry {
                name: def.name.clone(),
                description: def.description.clone().unwrap_or_default(),
                source: def.source,
                color: def.color,
                is_overridden,
                source_path: loaded.path.clone(),
            }
        })
        .collect();
    AgentsDialogPayload { entries }
}

fn render(args: &str, paths: AgentSearchPaths) -> crate::Result<String> {
    let mut store = AgentDefinitionStore::new(BuiltinAgentCatalog::interactive(), paths.clone());
    store.load();
    let snapshot = store.snapshot();
    let report = store.last_report();

    let (cmd, rest) = match args.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (args, ""),
    };

    Ok(match cmd {
        "" | "list" => render_list(&snapshot),
        "show" => render_show(&snapshot, rest),
        "paths" => render_paths(&paths),
        "validate" => render_validate(report),
        "reload" => render_reload(&snapshot),
        // `/agents <name>` is the shortcut: the flat-text equivalent is to
        // surface that agent's detail page directly.
        other if snapshot.find_active(other).is_some() => render_show(&snapshot, other),
        other => format!(
            "Unknown /agents subcommand: {other}\n\nUsage: /agents [list|show <name>|paths|validate|reload]"
        ),
    })
}

fn render_list(snapshot: &coco_subagent::AgentCatalogSnapshot) -> String {
    if snapshot.active_count() == 0 {
        return "No agents found.\n\
                Place markdown agent definitions in ~/.coco/agents (user) \
                or .coco/agents (project)."
            .to_string();
    }

    let mut out = format!("{} agent(s):\n\n", snapshot.active_count());
    for def in snapshot.active() {
        let model = def.model.as_deref().unwrap_or("inherit");
        let desc = def.description.as_deref().unwrap_or("(no description)");
        let source = def.source.as_str();
        out.push_str(&format!("  {}  [{source} · {model}]\n", def.name));
        out.push_str(&format!("    {desc}\n"));
    }
    // Opens an interactive 2-level menu (list →
    // View/Edit/Delete submenu) in the TUI. Flat text equivalents — keeps SDK + TUI
    // text-mode honest until a real overlay lands.
    out.push_str(
        "\nDetails: /agents show <name>  (or just /agents <name>)\n\
         Manage:  /agents reload | /agents validate | /agents paths\n\
         Note:    edit/delete from the TUI overlay are not yet ported — \
         edit the markdown files directly in the agents directories.",
    );
    out
}

fn render_show(snapshot: &coco_subagent::AgentCatalogSnapshot, name: &str) -> String {
    if name.is_empty() {
        return "Usage: /agents show <name>".to_string();
    }
    let Some(def) = snapshot.find_active(name) else {
        return format!("No active agent named: {name}");
    };

    let mut out = format!("# {}\n\n", def.name);
    out.push_str(&format!("Source:        {}\n", def.source.as_str()));
    if let Some(desc) = &def.description {
        out.push_str(&format!("Description:   {desc}\n"));
    }
    if let Some(model) = &def.model {
        out.push_str(&format!("Model:         {model}\n"));
    }
    if let Some(turns) = def.max_turns {
        out.push_str(&format!("Max turns:     {turns}\n"));
    }
    if def.background {
        out.push_str("Background:    true\n");
    }
    match &def.allowed_tools {
        coco_types::ToolAllowList::Wildcard => {
            // Wildcard = "all tools". Omit the line (only emit when an explicit
            // list is set).
        }
        coco_types::ToolAllowList::Explicit(list) if !list.is_empty() => {
            out.push_str(&format!("Tools:         {}\n", list.join(", ")));
        }
        coco_types::ToolAllowList::Explicit(_) => {}
    }
    if !def.disallowed_tools.is_empty() {
        out.push_str(&format!(
            "Disallowed:    {}\n",
            def.disallowed_tools.join(", ")
        ));
    }
    if !def.mcp_servers.is_empty() {
        let formatted = def
            .mcp_servers
            .iter()
            .map(|spec| match spec {
                coco_types::AgentMcpServerSpec::Name(s) => s.clone(),
                coco_types::AgentMcpServerSpec::Inline(map) => map
                    .keys()
                    .next()
                    .map(|n| format!("{n} (inline)"))
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("MCP servers:   {formatted}\n"));
    }
    if let Some(ts) = &def.pending_snapshot_update {
        // The CLI bootstrap (app/cli) wires the inspector that
        // populates this field; the slash command displays it when
        // present. `None` when no inspector is wired or the local
        // memory is already in sync.
        out.push_str(&format!("Pending sync:  {ts}\n"));
    }
    if let Some(prompt) = &def.initial_prompt {
        let preview = prompt.lines().take(10).collect::<Vec<_>>().join("\n");
        out.push_str(&format!("\nPrompt preview:\n{preview}\n"));
        if prompt.lines().count() > 10 {
            out.push_str("...\n");
        }
    }
    out
}

/// `/agents reload`: re-scan disk and report what's there now, but be
/// honest that the *engine's* live agent registry is loaded once at
/// session startup. The snapshot we just rendered reflects current disk
/// truth; mid-session edits affect the next session.
///
/// The interactive overlay reloads against the live registry — Rust doesn't
/// expose a thread-safe handle to the live store yet, so we surface the
/// deferral instead of pretending.
fn render_reload(snapshot: &coco_subagent::AgentCatalogSnapshot) -> String {
    let mut out = String::from(
        "Re-scanned agent definition directories. \
         Note: the engine's live agent registry is loaded once at session \
         start — disk-only changes (added / removed / edited markdown agents) \
         take effect on the next session. /agents list and /agents show \
         reflect current disk state.\n\n",
    );
    out.push_str(&render_list(snapshot));
    out
}

fn render_paths(paths: &AgentSearchPaths) -> String {
    let mut out = String::from("Agent search paths (later sources override earlier):\n\n");
    out.push_str("  built-in     (compiled-in catalog)\n");
    for d in &paths.plugin_dirs {
        out.push_str(&format!(
            "  plugin       {} ({})\n",
            d.dir.display(),
            d.plugin_name
        ));
    }
    if let Some(d) = &paths.user_dir {
        out.push_str(&format!("  user         {}\n", d.display()));
    }
    for d in &paths.project_dirs {
        out.push_str(&format!("  project      {}\n", d.display()));
    }
    for d in &paths.flag_dirs {
        out.push_str(&format!("  flag         {}\n", d.display()));
    }
    for d in &paths.policy_dirs {
        out.push_str(&format!("  policy       {}\n", d.display()));
    }
    out
}

fn render_validate(report: &coco_subagent::AgentLoadReport) -> String {
    if report.is_silent() {
        return "All agent definitions loaded with no warnings.".to_string();
    }
    let mut out = String::new();
    if !report.failed.is_empty() {
        out.push_str(&format!("{} failed:\n", report.failed.len()));
        for diag in &report.failed {
            out.push_str(&format!("  {}\n    {}\n", diag.path.display(), diag.error));
        }
    }
    if !report.warnings.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{} warning(s):\n", report.warnings.len()));
        for diag in &report.warnings {
            out.push_str(&format!("  {}\n    {}\n", diag.path.display(), diag.error));
        }
    }
    out
}

/// Standard slash-command agent search paths: `~/.coco/agents` (user) plus
/// `<cwd>/.coco/agents` (project). Mirrors the CLI helper of the same
/// shape — kept here so the handler stays self-contained.
fn standard_search_paths(config_home: &Path, cwd: &Path) -> AgentSearchPaths {
    let plugins = coco_plugins::load_enabled_plugins(config_home, cwd);
    let plugin_dirs = coco_plugins::plugin_agent_dirs(&plugins)
        .into_iter()
        .map(
            |(plugin_name, dir)| coco_subagent::definition_store::PluginAgentDir {
                plugin_name,
                dir,
            },
        )
        .collect();
    AgentSearchPaths {
        user_dir: Some(config_home.join("agents")),
        project_dirs: vec![cwd.join(".coco").join("agents")],
        flag_dirs: Vec::new(),
        policy_dirs: Vec::new(),
        plugin_dirs,
    }
}

#[cfg(test)]
#[path = "agents.test.rs"]
mod tests;
