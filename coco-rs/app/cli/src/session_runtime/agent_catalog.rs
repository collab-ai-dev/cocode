use std::sync::Arc;

use super::SessionRuntime;

impl SessionRuntime {
    /// Re-scan the configured agent search paths and replace the
    /// in-memory catalog snapshot. Subsequent per-turn engines built
    /// via [`Self::wire_engine`] pick up the new snapshot; engines
    /// already in flight keep the snapshot they captured at wire time.
    /// Borrow a stable pointer to the active agent catalog snapshot.
    /// The returned `Arc` is a pointer clone — readers continue to
    /// observe the snapshot they cloned even if [`reload_agent_catalog`]
    /// swaps in a new one mid-read. Callers (TUI bootstrap, `/agents`
    /// renderer) should re-call this for each fresh read rather than
    /// caching the result long-term.
    /// Uses the `RwLock<Arc<...>>` pattern to make `reload_agent_catalog`
    /// an atomic swap with no observer drift.
    pub async fn agent_catalog_snapshot(&self) -> Arc<coco_subagent::AgentCatalogSnapshot> {
        self.agent_catalog_resources
            .agent_catalog
            .read()
            .await
            .clone()
    }

    /// Triggered by `/agents reload`, `/reload-plugins`, and the
    /// future agent-dir file watcher.
    pub async fn reload_agent_catalog(&self) {
        let catalog = self.agent_catalog_resources.builtin_agent_catalog;
        let paths = self
            .agent_catalog_resources
            .agent_search_paths
            .read()
            .await
            .clone();
        let cwd = self.current_cwd().read().await.clone();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let auto_memory_enabled = self.runtime_config().memory_activation.active;
        // Clone the SDK-supplied agents Vec into the worker. After
        // `set_sdk_supplied_agents` populates the slot, every reload
        // picks up the same set as additional FlagSettings entries.
        // The Vec lives across `session/start` → `session/archive`
        // cycles so a single SDK connection's `initialize` payload
        // survives the whole connection lifetime.
        let sdk_agents = self
            .agent_catalog_resources
            .sdk_supplied_agents
            .read()
            .await
            .clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let mut store = coco_subagent::AgentDefinitionStore::new(catalog, paths);
            store.set_snapshot_inspector(Some(
                coco_memory::agent_memory_snapshot::build_pending_inspector(cwd, home),
            ));
            store.set_auto_memory_enabled(auto_memory_enabled);
            store.load();
            // Inject SDK-pushed agents AFTER on-disk load so they
            // participate in source-precedence resolution (FlagSettings
            // > ProjectSettings > UserSettings > Plugin > BuiltIn).
            // The store re-applies precedence on each `insert_definition`,
            // so an SDK agent with the same `agent_type` as a built-in
            // overrides the built-in.
            for def in sdk_agents {
                store.insert_definition(def);
            }
            store.snapshot()
        })
        .await
        .ok();
        if let Some(snapshot) = snapshot {
            *self.agent_catalog_resources.agent_catalog.write().await = snapshot;
        }
    }

    /// Replace the set of SDK-supplied agent definitions used by every
    /// future catalog (re)load. Called by the SDK `initialize` handler
    /// when the client pushes `initialize.agents`.
    /// Triggers an immediate `reload_agent_catalog()` so the new agents
    /// land in the active snapshot before the next `turn/start` (the
    /// engine snapshots the catalog when wiring per-turn).
    pub async fn set_sdk_supplied_agents(&self, agents: Vec<coco_types::AgentDefinition>) {
        let count = agents.len();
        {
            let mut slot = self
                .agent_catalog_resources
                .sdk_supplied_agents
                .write()
                .await;
            *slot = agents;
        }
        self.reload_agent_catalog().await;
        tracing::info!(
            target: "coco::session_runtime",
            count,
            "SDK-supplied agents applied; agent catalog reloaded"
        );
    }

    /// Cheap pointer-clone of the active catalog snapshot. The returned
    /// `Arc` is stable for the lifetime of the caller — a concurrent
    /// reload swaps the inner `Arc` but doesn't invalidate handles
    /// previously taken.
    pub async fn current_agent_catalog(&self) -> Arc<coco_subagent::AgentCatalogSnapshot> {
        self.agent_catalog_resources
            .agent_catalog
            .read()
            .await
            .clone()
    }

    /// The live catalog handle (shared `Arc<RwLock<..>>`), for consumers that
    /// need to read the *current* catalog at call time (e.g. `resume_agent`
    /// re-resolving an `AgentDefinition` after a `/agents reload`).
    pub fn agent_catalog_handle(
        &self,
    ) -> Arc<tokio::sync::RwLock<Arc<coco_subagent::AgentCatalogSnapshot>>> {
        self.agent_catalog_resources.agent_catalog.clone()
    }
}
