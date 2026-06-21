//! `coco agents` — list discovered agent definitions.
//!
//! Walks the standard agent dirs and prints a flat list. Discovery
//! sources: `coco-subagent` catalog (built-ins + per-source markdown loaders).

use anyhow::Result;

use coco_cli::paths::standard_agent_search_paths;
use coco_config::global_config;

pub async fn run_agents_subcommand() -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_home = global_config::config_home();
    let paths = standard_agent_search_paths(&config_home, &cwd);
    let mut store = coco_subagent::AgentDefinitionStore::new(
        coco_subagent::BuiltinAgentCatalog::interactive(),
        paths.clone(),
    );
    // Surface `pendingSnapshotUpdate` per definition so `coco agents` can
    // flag drift between project snapshots and local memory dirs.
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    store.set_snapshot_inspector(Some(
        coco_memory::agent_memory_snapshot::build_pending_inspector(cwd, home),
    ));
    store.load();
    let snapshot = store.snapshot();

    if snapshot.active_count() == 0 {
        let searched: Vec<String> = paths
            .user_dir
            .iter()
            .chain(paths.project_dirs.iter())
            .map(|p| p.display().to_string())
            .collect();
        println!("No agents found.");
        println!("Searched: {}", searched.join(", "));
        return Ok(());
    }

    let mut agents: Vec<&coco_types::AgentDefinition> = snapshot.active().collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    println!("{} agent(s):", agents.len());
    for agent in &agents {
        let model = agent.model.as_deref().unwrap_or("inherit");
        let desc = agent.description.as_deref().unwrap_or("(no description)");
        println!("  {} · {model}  — {desc}", agent.name);
        // Flags definitions whose project snapshot is newer than the
        // synced local memory. Snapshots are auto-applied at session
        // bootstrap (so the field is mostly informational outside the
        // CLI listing), but surfacing it here lets the user see which
        // agents drifted between launches.
        if let Some(ts) = &agent.pending_snapshot_update {
            println!("      ↳ pending memory snapshot update from {ts}");
        }
    }
    Ok(())
}
