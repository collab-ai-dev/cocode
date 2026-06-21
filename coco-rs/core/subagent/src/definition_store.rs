//! Central store for agent definitions. Built-ins + per-source loaders feed
//! the store; `snapshot()` returns an immutable per-turn view.
//!
//! `loadAgentsDir.ts:193-221` `getActiveAgentsFromList`.
//!
//! Sync-only: `load()` and `reload()` walk the filesystem with `std::fs`.
//! No tokio, no watcher — `app/state` owns the watcher and calls
//! `reload()` on file changes.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use coco_frontmatter::parse;
use coco_types::{AgentDefinition, AgentSource, AgentTypeId, MemoryScope};

use crate::builtins::{BuiltinAgentCatalog, builtin_definitions};
use crate::frontmatter::parse_agent_markdown;
use crate::snapshot::AgentCatalogSnapshot;
use crate::validation::{AgentDefinitionValidator, ValidationDiagnostic, ValidationError};

/// Maximum size for an agent markdown file. `loadAgentsDir.ts` rejects
/// files over 1 MiB so a malformed agent never bloats the prompt. Match
/// the limit verbatim — files larger than this are silently skipped with
/// a debug log so the loader stays robust on misconfigured workspaces.
const MAX_AGENT_FILE_SIZE_BYTES: u64 = 1024 * 1024;

/// Filename comparator for deterministic agent enumeration. `read_dir`
/// returns OS-dependent order (inode order on ext4, alphabetic on btrfs,
/// arbitrary on Windows/APFS). Sort by file path so same-priority
/// collisions resolve identically across platforms.
///
/// Walks the directory two levels deep (`loadAgentsDir.ts` uses
/// `walkdir({ max_depth: 2 })` so an `agents/<group>/foo.md` layout is
/// supported alongside `agents/foo.md`). Files larger than
/// [`MAX_AGENT_FILE_SIZE_BYTES`] are skipped with a `debug!` log so the
/// loader can never be DoSed by a stray binary that happens to end in
/// `.md`.
fn sorted_md_paths(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_md_paths(dir, 0, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_md_paths(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    // Walks two levels — root entries (depth 0) and one nested
    // subdirectory (depth 1). Anything deeper is ignored.
    const MAX_DEPTH: usize = 1;

    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if depth < MAX_DEPTH {
                let _ = collect_md_paths(&path, depth + 1, out);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        match entry.metadata() {
            Ok(meta) if meta.len() > MAX_AGENT_FILE_SIZE_BYTES => {
                tracing::debug!(
                    target: "coco_subagent",
                    path = %path.display(),
                    size = meta.len(),
                    cap = MAX_AGENT_FILE_SIZE_BYTES,
                    "skipping oversized agent file"
                );
                continue;
            }
            Ok(_) => out.push(path),
            Err(err) => {
                tracing::debug!(
                    target: "coco_subagent",
                    path = %path.display(),
                    error = %err,
                    "metadata failed; skipping"
                );
            }
        }
    }
    Ok(())
}

/// One loaded definition with its provenance recorded.
///
/// Per-file warnings live on `AgentLoadReport.warnings` (paired with the
/// path); they are not duplicated here to avoid two diverging copies after
/// `insert_definition` and similar mutators.
#[derive(Debug, Clone)]
pub struct LoadedAgentDefinition {
    pub definition: AgentDefinition,
    /// Absolute file path for markdown agents, `None` for in-process / built-in.
    pub path: Option<PathBuf>,
}

/// What `AgentDefinitionStore::load()` returns alongside the snapshot —
/// useful for `/agents validate` and bootstrap diagnostics.
#[derive(Debug, Default, Clone)]
pub struct AgentLoadReport {
    pub failed: Vec<ValidationDiagnostic>,
    pub warnings: Vec<ValidationDiagnostic>,
}

impl AgentLoadReport {
    /// No diagnostics at all — every file loaded with no issues.
    pub fn is_silent(&self) -> bool {
        self.failed.is_empty() && self.warnings.is_empty()
    }

    /// No load failures. Recoverable warnings (e.g. invalid color dropped)
    /// do not count as failures because the affected definition still
    /// loaded and is usable.
    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }
}

/// Where to look for markdown agent files. Order is informational only —
/// precedence is driven by `AgentSource::priority`, not directory order.
#[derive(Debug, Clone, Default)]
pub struct AgentSearchPaths {
    pub user_dir: Option<PathBuf>,
    pub project_dirs: Vec<PathBuf>,
    pub flag_dirs: Vec<PathBuf>,
    pub policy_dirs: Vec<PathBuf>,
    /// Plugin-contributed agent directories. Each carries the owning plugin's
    /// name so loaded agents are namespaced `<plugin>:<agent>`. The caller
    /// (`app/cli`) maps a plugin's `agents/` dir + manifest `agents` dirs
    /// to these.
    pub plugin_dirs: Vec<PluginAgentDir>,
}

/// A plugin's agent directory plus the plugin name used to namespace the
/// agents loaded from it.
#[derive(Debug, Clone)]
pub struct PluginAgentDir {
    pub plugin_name: String,
    pub dir: PathBuf,
}

impl AgentSearchPaths {
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Per-`(agent_type, memory_scope)` snapshot inspector — returns the
/// snapshot timestamp this agent's local memory dir is currently
/// behind, or `None` if the memory is up-to-date / no snapshot is
/// published. Caller (CLI bootstrap) wires this to
/// `coco_memory::agent_memory_snapshot::check_agent_memory_snapshot`
/// so the pure-logic crate stays free of `coco-memory` deps.
///
/// `memory_scope` is the agent's declared
/// [`AgentDefinition::memory_scope`]. The scope is used verbatim;
/// agents without a declared scope skip the inspector call.
///
/// `loadAgentsDir.ts:262-294` calls
/// `checkAgentMemorySnapshot(agentType, scope)` and sets
/// `pendingSnapshotUpdate` on the definition when the result is
/// `prompt-update`. The Rust loader does the same lookup but defers
/// the IO to this closure so `coco-subagent` stays a pure-logic crate.
///
/// The closure is called once per active agent_type during
/// [`AgentDefinitionStore::load`] / `reload`. Invocation is sync —
/// runtime callers should pre-resolve any async work or use blocking
/// IO inside the closure.
pub type SnapshotInspectorFn = Box<dyn Fn(&str, MemoryScope) -> Option<String> + Send + Sync>;

/// Aggregates built-ins, plugins, user, project, flag, and policy agents
/// into a single catalog with correct source-precedence ordering.
pub struct AgentDefinitionStore {
    catalog: BuiltinAgentCatalog,
    paths: AgentSearchPaths,
    snapshot: Arc<AgentCatalogSnapshot>,
    last_report: AgentLoadReport,
    snapshot_inspector: Option<SnapshotInspectorFn>,
    /// When true, post-process every loaded definition to auto-inject
    /// `Read` / `Edit` / `Write` into `allowed_tools` for agents that
    /// declare a `memory_scope`. `loadAgentsDir.ts:455-467,662-674`
    /// does this at parse time when `isAutoMemoryEnabled()` is true.
    /// The injection is a no-op for wildcard agents (empty
    /// `allowed_tools` = "use default" in coco-rs) — matches the
    /// `tools !== undefined` guard.
    auto_memory_enabled: bool,
}

impl AgentDefinitionStore {
    /// Construct an empty store. Call `load()` once to populate the snapshot.
    pub fn new(catalog: BuiltinAgentCatalog, paths: AgentSearchPaths) -> Self {
        Self {
            catalog,
            paths,
            snapshot: Arc::new(AgentCatalogSnapshot::new(BTreeMap::new(), Vec::new())),
            last_report: AgentLoadReport::default(),
            snapshot_inspector: None,
            auto_memory_enabled: false,
        }
    }

    /// Toggle auto-memory tool injection. When `true`, every agent with
    /// a non-empty `allowed_tools` and a declared `memory_scope` has
    /// `Read` / `Edit` / `Write` ensured present in its allow-list at
    /// load time. `loadAgentsDir.ts:455-467` runs the same transform
    /// when `isAutoMemoryEnabled()` is true. Caller (CLI bootstrap)
    /// reads `RuntimeConfig.features.enabled(AutoMemory)` and forwards
    /// the bool here. Off by default so the pure-logic crate doesn't
    /// pre-suppose a feature surface.
    pub fn set_auto_memory_enabled(&mut self, enabled: bool) {
        self.auto_memory_enabled = enabled;
    }

    /// Install a snapshot inspector that decorates each loaded
    /// definition's `pending_snapshot_update` field. CLI bootstrap
    /// wires this to `coco_memory::agent_memory_snapshot`; the
    /// pure-logic crate calls the closure once per active agent_type
    /// during `load()` / `reload()`.
    ///
    /// Pass `None` to unset (returns the previous value, if any).
    pub fn set_snapshot_inspector(
        &mut self,
        inspector: Option<SnapshotInspectorFn>,
    ) -> Option<SnapshotInspectorFn> {
        std::mem::replace(&mut self.snapshot_inspector, inspector)
    }

    /// Returns the current snapshot. Cheap pointer clone — no per-turn
    /// allocations even when callers re-snapshot every turn.
    pub fn snapshot(&self) -> Arc<AgentCatalogSnapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn last_report(&self) -> &AgentLoadReport {
        &self.last_report
    }

    pub fn catalog(&self) -> BuiltinAgentCatalog {
        self.catalog
    }

    pub fn paths(&self) -> &AgentSearchPaths {
        &self.paths
    }

    /// Walk built-ins and configured search paths, build a fresh snapshot.
    pub fn load(&mut self) -> &AgentLoadReport {
        let mut all: Vec<LoadedAgentDefinition> = Vec::new();
        let mut failed: Vec<ValidationDiagnostic> = Vec::new();
        let mut warnings: Vec<ValidationDiagnostic> = Vec::new();

        for def in builtin_definitions(self.catalog) {
            all.push(LoadedAgentDefinition {
                definition: def,
                path: None,
            });
        }

        // Inode dedup: a symlinked agent file under one source (e.g.
        // `~/.coco/agents/foo.md` -> `<project>/.coco/agents/foo.md`)
        // would otherwise parse twice and double-count in the source-
        // precedence map. `loadAgentsDir.ts:159-172` keys the dedup on
        // `(dev, ino)`. Same here on Unix; Windows skips because
        // `MetadataExt::dev/ino` aren't portable — Windows users get
        // the path-based sort behaviour we always had.
        let mut seen: HashSet<(u64, u64)> = HashSet::new();
        // Plugin agents first (lowest precedence after built-ins): each dir
        // carries its plugin name so agents are namespaced `<plugin>:<agent>`
        // and the plugin security gate is applied.
        for ps in &self.paths.plugin_dirs {
            collect_dir(
                &ps.dir,
                AgentSource::Plugin,
                Some(&ps.plugin_name),
                &mut all,
                &mut failed,
                &mut warnings,
                &mut seen,
            );
        }
        let plan: [(&[PathBuf], AgentSource); 4] = [
            (self.paths.user_dir.as_slice(), AgentSource::UserSettings),
            (&self.paths.project_dirs, AgentSource::ProjectSettings),
            (&self.paths.flag_dirs, AgentSource::FlagSettings),
            (&self.paths.policy_dirs, AgentSource::PolicySettings),
        ];
        for (dirs, source) in plan {
            for dir in dirs {
                collect_dir(
                    dir,
                    source,
                    None,
                    &mut all,
                    &mut failed,
                    &mut warnings,
                    &mut seen,
                );
            }
        }

        let mut active = compute_active(&all);
        // Decorate each active definition with its pending-snapshot
        // status. The closure stays out of the pure-logic crate's IO
        // graph — caller wires it at bootstrap. Agents without a
        // declared `memory_scope` skip the call (matches the original,
        // which only invokes `checkAgentMemorySnapshot(agentType,
        // definition.memory)` when `memory` is set).
        if let Some(inspect) = self.snapshot_inspector.as_ref() {
            for (name, def) in active.iter_mut() {
                let Some(scope) = def.memory_scope else {
                    continue;
                };
                if let Some(timestamp) = inspect(name, scope) {
                    def.pending_snapshot_update = Some(timestamp);
                }
            }
        }
        // Auto-memory tool injection —
        // `loadAgentsDir.ts:455-467` runs this when `isAutoMemoryEnabled()`
        // and the agent declares a `memory` scope. Wildcard
        // (empty allow-list) skips the injection because the
        // `tools !== undefined` guard treats wildcard as "all tools
        // already".
        if self.auto_memory_enabled {
            for def in active.values_mut() {
                inject_memory_tools(def);
            }
        }
        self.snapshot = Arc::new(AgentCatalogSnapshot::new(active, all));
        self.last_report = AgentLoadReport { failed, warnings };
        &self.last_report
    }

    /// Manual refresh from disk. The watcher in `app/state` invokes this
    /// when an agent file changes.
    pub fn reload(&mut self) -> &AgentLoadReport {
        self.load()
    }

    /// Add a definition from an in-process source (SDK, CLI flag JSON).
    /// The store re-applies precedence after the insert.
    pub fn insert_definition(&mut self, def: AgentDefinition) {
        let mut all = self.snapshot.all().to_vec();
        all.push(LoadedAgentDefinition {
            definition: def,
            path: None,
        });
        let active = compute_active(&all);
        self.snapshot = Arc::new(AgentCatalogSnapshot::new(active, all));
    }
}

/// Namespace a plugin-sourced agent `<plugin>:<agent>` and strip the fields a
/// plugin agent is not trusted to declare. `loadPluginAgents` deliberately
/// drops `permissionMode` / `hooks` / `mcpServers` for plugin agents so a
/// plugin cannot escalate beyond install-time trust.
fn apply_plugin_namespace_and_gate(def: &mut AgentDefinition, plugin_name: &str) {
    let base = def.name.clone();
    let namespaced = format!("{plugin_name}:{base}");
    def.agent_type = AgentTypeId::Custom(namespaced.clone());
    def.name = namespaced;
    def.filename = Some(base);
    def.permission_mode = None;
    def.hooks = serde_json::Value::Null;
    def.mcp_servers = Vec::new();
}

fn collect_dir(
    dir: &Path,
    source: AgentSource,
    plugin_name: Option<&str>,
    all: &mut Vec<LoadedAgentDefinition>,
    failed: &mut Vec<ValidationDiagnostic>,
    warnings: &mut Vec<ValidationDiagnostic>,
    seen_inodes: &mut HashSet<(u64, u64)>,
) {
    let paths = match sorted_md_paths(dir) {
        Ok(paths) => paths,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(target: "coco_subagent", path=%dir.display(), error=%err, "read_dir failed");
            }
            return;
        }
    };

    for path in paths {
        // Skip symlink-equivalent files we already loaded from a
        // higher-priority source. `loadAgentsDir.ts:159-172`.
        if !record_inode_seen(&path, seen_inodes) {
            tracing::debug!(
                target: "coco_subagent",
                path = %path.display(),
                "skipping duplicate agent file (same dev/ino as an already-loaded path)"
            );
            continue;
        }
        match load_one(&path, source) {
            Ok((mut def, def_warnings)) => {
                if let Some(plugin) = plugin_name {
                    apply_plugin_namespace_and_gate(&mut def, plugin);
                }
                // Always surface frontmatter warnings, even when the
                // semantic validator rejects the definition.
                for w in &def_warnings {
                    warnings.push(ValidationDiagnostic::new(
                        path.clone(),
                        Some(def.name.clone()),
                        w.clone(),
                    ));
                }
                let semantic_errors = AgentDefinitionValidator::check(&def);
                if semantic_errors.is_empty() {
                    all.push(LoadedAgentDefinition {
                        definition: def,
                        path: Some(path.clone()),
                    });
                } else {
                    for err in semantic_errors {
                        failed.push(ValidationDiagnostic::new(
                            path.clone(),
                            Some(def.name.clone()),
                            err,
                        ));
                    }
                }
            }
            Err(diag) => failed.push(diag),
        }
    }
}

/// Record `(dev, ino)` for `path` and return `true` when this is the
/// first time we've seen the inode — `false` (skip) when a previous
/// higher-priority source already loaded the same file.
///
/// Always returns `true` on non-Unix platforms (the `MetadataExt`
/// trait that exposes `dev`/`ino` is Unix-only). Symlink dedup on
/// Windows is rare in practice and would need a different approach
/// (`GetFileInformationByHandle`'s volume serial + file index).
#[cfg(unix)]
fn record_inode_seen(path: &Path, seen: &mut HashSet<(u64, u64)>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else {
        // If we can't stat the file we can't dedup — let the caller
        // try to load it; the load_one error path will report the
        // real failure with a richer diagnostic.
        return true;
    };
    seen.insert((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn record_inode_seen(_path: &Path, _seen: &mut HashSet<(u64, u64)>) -> bool {
    true
}

fn load_one(
    path: &Path,
    source: AgentSource,
) -> Result<(AgentDefinition, Vec<ValidationError>), ValidationDiagnostic> {
    let raw = std::fs::read_to_string(path).map_err(|err| {
        ValidationDiagnostic::new(
            path.to_path_buf(),
            None,
            ValidationError::Io {
                message: err.to_string(),
            },
        )
    })?;
    let parsed = parse(&raw);
    parse_agent_markdown(path, &parsed.content, &parsed.data, source)
        .map_err(|err| ValidationDiagnostic::new(path.to_path_buf(), None, err.into()))
}

/// Auto-inject `Read`, `Edit`, `Write` into `def.allowed_tools` when
/// the agent declares a `memory_scope` AND has an `Explicit` allow-list.
/// **`Wildcard` allow-lists are skipped** — the agent already sees every
/// tool, so injection is meaningless (and the type system, via
/// [`ToolAllowList::as_explicit_mut`], makes it unrepresentable).
/// `loadAgentsDir.ts:455-467,662-674`. Idempotent — running the
/// function repeatedly leaves the tool list unchanged after the first
/// call, so future re-loads with auto-memory still on don't duplicate
/// entries.
fn inject_memory_tools(def: &mut AgentDefinition) {
    use coco_types::ToolName;
    if def.memory_scope.is_none() {
        return;
    }
    let Some(list) = def.allowed_tools.as_explicit_mut() else {
        // Wildcard — every tool already visible.
        return;
    };
    // Inject in [Write, Edit, Read] order (loadAgentsDir.ts:458-462,
    // 665-669); keep byte-faithful so the prompt-cache key is stable.
    for tool in [ToolName::Write, ToolName::Edit, ToolName::Read] {
        let name = tool.as_str();
        if !list.iter().any(|t| t == name) {
            list.push(name.to_owned());
        }
    }
}

/// Apply source precedence: later (higher-or-equal-priority) source wins.
/// Equal-priority later wins matches `Map.set` semantics in
/// `getActiveAgentsFromList`. Caller controls iteration order via the
/// `paths` field on `AgentSearchPaths`; intra-directory order is sorted
/// in `sorted_md_paths` for cross-OS determinism.
fn compute_active(all: &[LoadedAgentDefinition]) -> BTreeMap<String, AgentDefinition> {
    let mut active: BTreeMap<String, AgentDefinition> = BTreeMap::new();
    for loaded in all {
        let priority = loaded.definition.source.priority();
        match active.get(&loaded.definition.name) {
            Some(existing) if existing.source.priority() > priority => {}
            _ => {
                active.insert(loaded.definition.name.clone(), loaded.definition.clone());
            }
        }
    }
    active
}
