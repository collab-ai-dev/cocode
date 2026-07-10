//! Project-scoped service/catalog preparation.
//!
//! This is the project-service slice of the Process/Project/Session split: a
//! per-project plugin catalog snapshot, project-rooted command,
//! skill, hook, MCP, LSP, output-style, and agent discovery, and the registry
//! that shares them across sessions in the same process.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::RwLockReadGuard;
use std::sync::RwLockWriteGuard;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use crate::workspace::git_root_for;

const PROJECT_SERVICES_IDLE_TTL: Duration = Duration::from_secs(60 * 60);
pub(crate) const PROJECT_SERVICES_IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Backing singleton for the interim app/cli `ProcessRuntime`.
///
/// Keep production access routed through `ProcessRuntime`; this accessor exists
/// so the startup-owned manager has a single process registry to wrap until the
/// planned `coco-app-runtime` extraction owns the field directly.
pub fn project_registry() -> &'static ProjectRegistry {
    static REGISTRY: OnceLock<ProjectRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ProjectRegistry::default)
}

/// Process-level owner for [`ProjectRegistry`] lifecycle work.
///
/// It keeps the idle-eviction task tied to an explicit startup-owned guard
/// instead of hiding background work inside the registry accessor.
pub struct ProjectRegistryManager {
    registry: &'static ProjectRegistry,
    idle_eviction_task: tokio::task::JoinHandle<()>,
}

impl ProjectRegistryManager {
    pub fn start_global() -> Self {
        Self::start(
            project_registry(),
            PROJECT_SERVICES_IDLE_TTL,
            PROJECT_SERVICES_IDLE_SWEEP_INTERVAL,
        )
    }

    pub fn start(
        registry: &'static ProjectRegistry,
        idle_for: Duration,
        sweep_interval: Duration,
    ) -> Self {
        registry.set_idle_ttl(idle_for);
        let idle_eviction_task = spawn_idle_eviction_loop(registry, sweep_interval);
        Self {
            registry,
            idle_eviction_task,
        }
    }

    pub fn registry(&self) -> &ProjectRegistry {
        self.registry
    }
}

impl Drop for ProjectRegistryManager {
    fn drop(&mut self) {
        self.idle_eviction_task.abort();
    }
}

fn spawn_idle_eviction_loop(
    registry: &'static ProjectRegistry,
    sweep_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(sweep_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            // Read the TTL live so a config value applied after startup
            // (`ProcessRuntime::set_project_services_idle_ttl`) takes effect.
            let evicted = registry.evict_idle(registry.effective_idle_ttl());
            if evicted > 0 {
                tracing::debug!(
                    target: "coco::project_services",
                    evicted,
                    "evicted idle project service entries"
                );
            }
        }
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ProjectRegistryKey {
    config_home: PathBuf,
    project_root: PathBuf,
}

impl ProjectRegistryKey {
    fn new(config_home: &Path, project_root: impl Into<PathBuf>) -> Self {
        Self {
            config_home: config_home.to_path_buf(),
            project_root: project_root.into(),
        }
    }
}

/// Cache of project-scoped service snapshots.
///
/// Loading is intentionally synchronous today, so holding the write lock during
/// a miss gives a simple single-flight guarantee: concurrent callers for the
/// same key share the first loaded `Arc<ProjectServices>`.
#[derive(Debug)]
pub struct ProjectRegistry {
    projects: RwLock<HashMap<ProjectRegistryKey, ProjectRegistryEntry>>,
    /// Idle-eviction grace period in seconds, from
    /// `server.project_services_idle_ttl_secs`. `-1` means "not configured"
    /// (resolution falls back to [`PROJECT_SERVICES_IDLE_TTL`]); `0` is a
    /// valid configured value meaning "evict as soon as unattached".
    idle_ttl_secs: AtomicI64,
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self {
            projects: RwLock::new(HashMap::new()),
            idle_ttl_secs: AtomicI64::new(-1),
        }
    }
}

impl ProjectRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the idle-eviction grace period (from resolved config).
    pub fn set_idle_ttl(&self, idle_for: Duration) {
        self.idle_ttl_secs
            .store(idle_for.as_secs() as i64, Ordering::Relaxed);
    }

    fn effective_idle_ttl(&self) -> Duration {
        match self.idle_ttl_secs.load(Ordering::Relaxed) {
            secs if secs < 0 => PROJECT_SERVICES_IDLE_TTL,
            secs => Duration::from_secs(secs as u64),
        }
    }

    pub fn get_or_load(
        &self,
        config_home: &Path,
        project_root: impl Into<PathBuf>,
    ) -> Arc<ProjectServices> {
        let key = ProjectRegistryKey::new(config_home, project_root);
        // Fast path: a cached entry whose project settings file is unchanged is
        // served as-is. If the file changed since the entry was built, fall
        // through to rebuild it under the write lock so the next session in this
        // project observes the current settings AND a freshly-loaded plugin
        // catalog (plugin enable/disable is a project-settings concern). Sessions
        // already holding the old `Arc` keep their own fold snapshot.
        if let Some(project) = self.read_projects().get(&key)
            && !project.services.config_is_stale()
        {
            return project.services.clone();
        }

        let mut projects = self.write_projects();
        Self::evict_idle_locked(&mut projects, self.effective_idle_ttl(), Instant::now());
        // Re-check under the write lock: another caller may have rebuilt it, or
        // the entry may still be fresh (the read-lock window raced an insert).
        let config_home = key.config_home.clone();
        let project_root = key.project_root.clone();
        let entry = match projects.entry(key) {
            Entry::Occupied(mut occupied) => {
                if occupied.get().services.config_is_stale() {
                    *occupied.get_mut() = ProjectRegistryEntry::new(Arc::new(
                        ProjectServices::load(&config_home, project_root),
                    ));
                }
                occupied.into_mut()
            }
            Entry::Vacant(vacant) => vacant.insert(ProjectRegistryEntry::new(Arc::new(
                ProjectServices::load(&config_home, project_root),
            ))),
        };
        entry.idle_since = None;
        entry.services.clone()
    }

    pub fn reload(
        &self,
        config_home: &Path,
        project_root: impl Into<PathBuf>,
    ) -> Arc<ProjectServices> {
        let key = ProjectRegistryKey::new(config_home, project_root);
        let project = Arc::new(ProjectServices::load(
            &key.config_home,
            key.project_root.clone(),
        ));
        self.write_projects()
            .insert(key, ProjectRegistryEntry::new(project.clone()));
        project
    }

    /// Evict project services whose only remaining strong reference is the
    /// registry itself and whose idle grace period has elapsed.
    pub fn evict_idle(&self, idle_for: Duration) -> usize {
        let mut projects = self.write_projects();
        Self::evict_idle_locked(&mut projects, idle_for, Instant::now())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.read_projects().len()
    }

    fn evict_idle_locked(
        projects: &mut HashMap<ProjectRegistryKey, ProjectRegistryEntry>,
        idle_for: Duration,
        now: Instant,
    ) -> usize {
        let before = projects.len();
        projects.retain(|_, entry| {
            if Arc::strong_count(&entry.services) > 1 {
                entry.idle_since = None;
                return true;
            }
            match entry.idle_since {
                Some(idle_since) if now.duration_since(idle_since) >= idle_for => false,
                Some(_) => true,
                None => {
                    entry.idle_since = Some(now);
                    true
                }
            }
        });
        before.saturating_sub(projects.len())
    }

    fn read_projects(
        &self,
    ) -> RwLockReadGuard<'_, HashMap<ProjectRegistryKey, ProjectRegistryEntry>> {
        match self.projects.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_projects(
        &self,
    ) -> RwLockWriteGuard<'_, HashMap<ProjectRegistryKey, ProjectRegistryEntry>> {
        match self.projects.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[derive(Debug)]
struct ProjectRegistryEntry {
    services: Arc<ProjectServices>,
    idle_since: Option<Instant>,
}

impl ProjectRegistryEntry {
    fn new(services: Arc<ProjectServices>) -> Self {
        Self {
            services,
            idle_since: None,
        }
    }
}

/// Project-scoped services for one resolved project root.
#[derive(Debug)]
pub struct ProjectServices {
    config_snapshot: RwLock<ProjectConfigSnapshot>,
    catalog: ProjectCatalogSnapshot,
}

impl ProjectServices {
    pub fn load(config_home: &Path, project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        Self {
            config_snapshot: RwLock::new(ProjectConfigSnapshot::load(project_root.clone())),
            catalog: ProjectCatalogSnapshot::load(config_home, project_root),
        }
    }

    pub fn project_root(&self) -> &Path {
        self.catalog.project_root()
    }

    pub fn project_config_snapshot(&self) -> ProjectConfigSnapshot {
        match self.config_snapshot.read() {
            Ok(snapshot) => snapshot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Whether the project settings file backing this entry has changed on
    /// disk since the entry was built. Drives the [`ProjectRegistry`] rebuild
    /// decision so a stale plugin catalog is refreshed for the next session.
    fn config_is_stale(&self) -> bool {
        let snapshot = match self.config_snapshot.read() {
            Ok(snapshot) => snapshot,
            Err(poisoned) => poisoned.into_inner(),
        };
        snapshot.has_changed()
    }

    pub fn output_style_sources(&self) -> Vec<coco_output_styles::PluginOutputStyleSource> {
        self.catalog.output_style_sources()
    }

    pub fn agent_search_paths(
        &self,
        config_home: &Path,
        cwd: &Path,
    ) -> coco_subagent::definition_store::AgentSearchPaths {
        self.catalog.agent_search_paths(config_home, cwd)
    }

    pub fn mcp_servers(
        &self,
        config_home: &Path,
        session_cwd: &Path,
    ) -> Vec<coco_mcp::ScopedMcpServerConfig> {
        let mut servers = coco_mcp::McpConfigLoader::load_with_roots(
            coco_mcp::McpConfigRoots {
                project_root: self.project_root(),
                session_cwd,
            },
            config_home,
        );
        servers.extend(self.plugin_mcp_servers());
        servers
    }

    pub fn plugin_mcp_servers(&self) -> Vec<coco_mcp::ScopedMcpServerConfig> {
        self.catalog.plugin_mcp_servers()
    }

    pub fn lsp_servers(&self) -> coco_lsp::LspServersConfig {
        self.catalog.lsp_servers()
    }

    pub fn build_skill_manager(
        &self,
        config_home: &Path,
        session_cwd: &Path,
        gates: &coco_skills::SkillLoadGates,
    ) -> coco_skills::SkillManager {
        let manager = coco_skills::build_session_skill_manager(config_home, session_cwd, gates);
        self.catalog
            .register_project_plugin_skills(config_home, &manager);
        manager
    }

    pub fn register_plugin_hooks(&self, registry: &coco_hooks::HookRegistry) -> usize {
        self.catalog.register_plugin_hooks(registry)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_command_registry(
        &self,
        skill_manager: &coco_skills::SkillManager,
        user_type: coco_types::UserType,
        features: coco_types::Features,
        loop_config: coco_config::LoopConfig,
        session_cwd: PathBuf,
        user_home: PathBuf,
        managed_root: Option<PathBuf>,
        skill_overrides: &coco_config::SkillOverrideTiers,
    ) -> coco_commands::CommandRegistry {
        coco_commands::build_command_registry(
            skill_manager,
            self.catalog.plugins(),
            user_type,
            features,
            loop_config,
            session_cwd,
            user_home,
            managed_root,
            skill_overrides,
        )
    }
}

#[cfg(test)]
#[path = "project_services.test.rs"]
mod tests;

/// Project-rooted config files tracked by the project-service cache.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProjectConfigSnapshot {
    settings_path: PathBuf,
    settings_fingerprint: FileFingerprint,
}

impl ProjectConfigSnapshot {
    fn load(project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        let settings_path = coco_config::global_config::project_settings_path(&project_root);
        let settings_fingerprint = FileFingerprint::for_path(&settings_path);
        Self {
            settings_path,
            settings_fingerprint,
        }
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    pub fn has_changed(&self) -> bool {
        FileFingerprint::for_path(&self.settings_path) != self.settings_fingerprint
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum FileFingerprint {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
}

impl FileFingerprint {
    fn for_path(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Self::Present {
                len: metadata.len(),
                modified: metadata.modified().ok(),
            },
            _ => Self::Missing,
        }
    }
}

/// Project-scoped plugin catalog loaded against a resolved project root.
#[derive(Debug, Clone)]
struct ProjectCatalogSnapshot {
    project_root: PathBuf,
    plugins: Vec<coco_plugins::loader::LoadedPluginV2>,
}

impl ProjectCatalogSnapshot {
    pub fn load(config_home: &Path, project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        let plugins = coco_plugins::load_enabled_plugins(config_home, &project_root);
        Self {
            project_root,
            plugins,
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    fn plugins(&self) -> &[coco_plugins::loader::LoadedPluginV2] {
        &self.plugins
    }

    pub fn output_style_sources(&self) -> Vec<coco_output_styles::PluginOutputStyleSource> {
        self.plugins
            .iter()
            .map(coco_output_styles::PluginOutputStyleSource::from_loaded_plugin)
            .collect()
    }

    pub fn agent_search_paths(
        &self,
        config_home: &Path,
        cwd: &Path,
    ) -> coco_subagent::definition_store::AgentSearchPaths {
        standard_agent_search_paths_with_plugins(config_home, cwd, &self.plugins)
    }

    pub fn plugin_mcp_servers(&self) -> Vec<coco_mcp::ScopedMcpServerConfig> {
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = self.plugins.iter().collect();
        coco_plugins::mcp_bridge::extract_mcp_servers_from_plugins(&plugin_refs)
    }

    pub fn lsp_servers(&self) -> coco_lsp::LspServersConfig {
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = self.plugins.iter().collect();
        coco_plugins::lsp_bridge::extract_lsp_servers_from_plugins(&plugin_refs)
    }

    fn register_project_plugin_skills(
        &self,
        config_home: &Path,
        manager: &coco_skills::SkillManager,
    ) {
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = self.plugins.iter().collect();
        for skill in coco_plugins::skill_bridge::load_all_plugin_skills_v2(&plugin_refs) {
            manager.register(skill);
        }

        coco_plugins::builtins::init_builtin_plugins();
        for skill in coco_plugins::builtin_plugin_skills(config_home) {
            manager.register(skill);
        }
    }

    fn register_plugin_hooks(&self, registry: &coco_hooks::HookRegistry) -> usize {
        let plugin_refs: Vec<&coco_plugins::loader::LoadedPluginV2> = self.plugins.iter().collect();
        coco_plugins::hook_bridge::register_plugin_hooks_v2(registry, &plugin_refs);
        plugin_refs.len()
    }
}

/// Standard agent search paths with project plugin contributions.
pub fn standard_agent_search_paths_with_plugins(
    config_home: &Path,
    cwd: &Path,
    plugins: &[coco_plugins::loader::LoadedPluginV2],
) -> coco_subagent::definition_store::AgentSearchPaths {
    let mut project_dirs = vec![
        cwd.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("agents"),
    ];

    if let Some(canonical_root) = coco_git::find_canonical_git_root(cwd) {
        let worktree_agents_dir = cwd
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("agents");
        let worktree_root = git_root_for(cwd);
        let worktree_has_agents = worktree_agents_dir.is_dir();
        let canonical_agents_dir = canonical_root
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("agents");
        if worktree_root.as_deref() != Some(canonical_root.as_path())
            && !worktree_has_agents
            && canonical_agents_dir.is_dir()
            && !project_dirs
                .iter()
                .any(|path| path == &canonical_agents_dir)
        {
            project_dirs.push(canonical_agents_dir);
        }
    }

    coco_subagent::definition_store::AgentSearchPaths {
        user_dir: Some(config_home.join("agents")),
        project_dirs,
        plugin_dirs: coco_plugins::plugin_agent_dirs(plugins)
            .into_iter()
            .map(
                |(plugin_name, dir)| coco_subagent::definition_store::PluginAgentDir {
                    plugin_name,
                    dir,
                },
            )
            .collect(),
        ..coco_subagent::definition_store::AgentSearchPaths::empty()
    }
}
