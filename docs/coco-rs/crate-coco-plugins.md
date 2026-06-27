# coco-plugins — Crate Plan

TS source: `plugins/`, `types/plugin.ts`, `utils/plugins/` (44 files, 20.5K LOC)

## Dependencies

```
coco-plugins depends on:
  - coco-types, coco-skills (SkillDefinition), coco-hooks (HooksSettings)
  - coco-config (Settings — plugins section as Value)
  - tokio, reqwest (marketplace fetching, git operations)
  - serde_json (plugin.json manifest parsing)

coco-plugins does NOT depend on:
  - coco-tools, coco-query, coco-inference, any app/ crate
```

## 1. Manifest Format

Plugin manifest is `plugin.json` (JSON, not TOML). Located at root of each plugin directory.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<PluginAuthor>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// apt-style inter-plugin dependency declarations.
    pub dependencies: Option<Vec<String>>,
    /// 9 contribution types.
    #[serde(default)]
    pub contributions: PluginContributions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    pub email: Option<String>,
    pub url: Option<String>,
}

/// All contribution types a plugin can provide.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginContributions {
    pub commands: Option<Vec<CommandContribution>>,
    pub agents: Option<Vec<AgentContribution>>,
    pub skills: Option<Vec<SkillDefinition>>,
    pub hooks: Option<HooksSettings>,
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    pub lsp_servers: Option<HashMap<String, LspServerConfig>>,
    pub output_styles: Option<Vec<OutputStyleContribution>>,
    pub settings: Option<HashMap<String, SettingSchemaEntry>>,
    pub user_config: Option<HashMap<String, Value>>,
    pub channels: Option<Vec<ChannelContribution>>,
}
```

## 2. Plugin Sources (7 types)

```rust
/// How a plugin is fetched and materialized on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginSource {
    /// Local filesystem path.
    Local { path: PathBuf },
    /// npm package (name + optional version).
    Npm { package: String, version: Option<String> },
    /// pip/PyPI package.
    Pip { package: String, version: Option<String> },
    /// Git repository with optional ref and SHA pinning.
    Git { url: String, r#ref: Option<String>, sha: Option<String> },
    /// GitHub shorthand: owner/repo.
    GitHub { owner: String, repo: String, r#ref: Option<String> },
    /// Sparse checkout of a subdirectory within a git monorepo.
    GitSubdir { url: String, subdir: String, r#ref: Option<String>, sha: Option<String> },
    /// Direct URL to a plugin archive.
    Url { url: String },
}
```

## 3. Plugin Scoping

Four scopes (priority order, highest-first), stored in `installed_plugins.json` V2:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginScope {
    /// Enterprise-managed plugins (policy-enforced, cannot be disabled by user).
    Managed,
    /// User-installed plugins (~/.coco/plugins/).
    User,
    /// Project-local plugins (.coco/plugins/).
    Project,
    /// Local development plugins (filesystem path).
    Local,
}
```

Higher-scoped contributions override lower when names collide. `Managed` plugins cannot be disabled or uninstalled by the user.

## 4. Marketplace System

Two-level hierarchy: marketplaces contain plugins, plugins contain contributions.

### marketplace.json Format

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    pub name: String,
    pub description: Option<String>,
    pub plugins: Vec<MarketplacePluginEntry>,
    /// If true, plugins removed from the manifest are force-uninstalled locally.
    #[serde(default)]
    pub force_remove_deleted_plugins: bool,
    /// List of marketplace names whose plugins may be declared as dependencies
    /// even though they are from a different marketplace.
    #[serde(default)]
    pub allow_cross_marketplace_dependencies_on: Vec<String>,
    /// Auto-update interval hint (e.g. "daily", "weekly").
    pub auto_update: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub source: PluginSource,
    pub version: Option<String>,
    pub required: Option<bool>,
}
```

### Marketplace Source Types (8)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MarketplaceSource {
    /// Direct URL to a marketplace.json.
    Url { url: String },
    /// GitHub repo containing marketplace.json at root.
    GitHub { owner: String, repo: String, r#ref: Option<String> },
    /// Git repository.
    Git { url: String, r#ref: Option<String> },
    /// npm package exporting marketplace.json.
    Npm { package: String },
    /// Local file path to marketplace.json.
    File { path: PathBuf },
    /// Directory of plugins (each subdirectory is a plugin).
    Directory { path: PathBuf },
    /// Host-pattern matching (enterprise: auto-assign marketplace for host).
    HostPattern { pattern: String, marketplace_url: String },
    /// Path-pattern matching (enterprise: auto-assign marketplace for project).
    PathPattern { pattern: String, marketplace_url: String },
    /// Inline marketplace settings.
    Settings { plugins: Vec<MarketplacePluginEntry> },
}
```

### Official Marketplace

`anthropics/claude-plugins-official` is auto-installed on first run. Enterprise policy can override or block.

## 5. Dependency Resolution

```rust
impl PluginDependencyResolver {
    /// DFS transitive closure with cycle detection.
    /// Traverses each plugin's `dependencies` field, resolving them
    /// across the installed set. If a cycle is detected, returns
    /// PluginError::DependencyCycle with the cycle path.
    pub fn resolve_transitive(
        plugins: &[InstalledPlugin],
    ) -> Result<Vec<ResolvedPlugin>, PluginError>;

    /// Load-time fixed-point demotion: after resolving, iteratively demote
    /// plugin scope to the maximum of its dependencies' scopes.
    /// Converges in O(depth) iterations.
    /// Example: a User-scoped plugin depending on a Project-scoped plugin
    /// is demoted to Project scope.
    pub fn verify_and_demote(
        resolved: &mut [ResolvedPlugin],
    ) -> Vec<DemotionWarning>;

    /// Build reverse-dependency map for uninstall warnings.
    /// Before removing plugin P, enumerate all plugins that transitively
    /// depend on P and warn the user.
    pub fn reverse_dependents(
        plugin_name: &str,
        resolved: &[ResolvedPlugin],
    ) -> Vec<String>;
}
```

Cross-marketplace dependencies are blocked by default. Only allowed when the source marketplace's `allow_cross_marketplace_dependencies_on` list includes the target marketplace name.

## 6. Security and Policy

```rust
impl PluginSecurity {
    /// Path traversal detection: reject plugin names or paths containing
    /// "..", absolute paths, or symlinks escaping the plugin root.
    pub fn validate_paths(manifest: &PluginManifest, root: &Path) -> Result<(), PluginError>;

    /// Official-name impersonation blocking:
    /// - Regex match against known official plugin name patterns
    /// - Non-ASCII homograph detection (confusable characters)
    /// - Rejects third-party plugins mimicking official names
    pub fn check_impersonation(name: &str) -> Result<(), PluginError>;

    /// Enterprise policy check.
    /// Evaluates: strict_known_marketplaces, blocked_marketplaces,
    /// strict_plugin_only_customization.
    pub fn is_plugin_blocked_by_policy(
        plugin: &InstalledPlugin,
        policy: &EnterprisePolicy,
    ) -> bool;
}

/// Enterprise policy fields relevant to plugins.
pub struct EnterprisePluginPolicy {
    /// Only allow plugins from known/approved marketplaces.
    pub strict_known_marketplaces: bool,
    /// Explicit blocklist of marketplace names or URLs.
    pub blocked_marketplaces: Vec<String>,
    /// If true, users cannot install plugins outside of managed scope.
    pub strict_plugin_only_customization: bool,
}
```

## MCPB Format (MCP Bundle)

```rust
/// MCPB = ZIP-format container for MCP server plugins.
/// Extensions: .mcpb, .dxt
/// Contains: manifest.json, extracted server binaries, optional config metadata.
///
/// Load pipeline:
/// 1. Download/extract ZIP to cache dir
/// 2. Parse manifest.json → McpbManifest
/// 3. Check user config requirements (configSchema)
/// 4. Generate MCP server config from manifest
///
/// Cache: content-addressed (SHA hash), stored at ~/.coco/plugins/mcpb-cache/
/// Metadata tracked per-source: source URL, content hash, extracted path, timestamps.
pub struct McpbLoadResult {
    pub manifest: Value,         // McpbManifest
    pub mcp_config: Value,       // generated MCP server config
    pub extracted_path: PathBuf,
    pub content_hash: String,
}

/// User config requirements for MCPB plugins.
/// Schema defines required config values; validation errors returned if missing.
pub enum McpbLoadStatus {
    Ready(McpbLoadResult),
    NeedsConfig {
        config_schema: HashMap<String, Value>,
        existing_config: HashMap<String, Value>,
        validation_errors: Vec<String>,
    },
}
```

## 7. Three-Layer Refresh Model

Plugin state flows through three layers on each session start and on settings change:

```
Layer 1: Intent
  enabledPlugins in settings.json
  User declares which plugins should be active.
     ↓
Layer 2: Materialization
  reconcile_marketplaces() clones/fetches to ~/.coco/plugins/
  Resolves sources, downloads, validates manifests, writes installed_plugins.json.
     ↓
Layer 3: Active
  refresh_active_plugins() loads into AppState
  Parses manifests, resolves dependencies, merges contributions into
  SkillManager + HookExecutor + McpServerRegistry + etc.
```

```rust
impl PluginManager {
    /// Layer 2: reconcile marketplace manifests with local disk.
    /// Clones new plugins, updates changed ones, removes deleted ones
    /// (if force_remove_deleted_plugins is set).
    pub async fn reconcile_marketplaces(
        &mut self,
        settings: &Settings,
    ) -> Result<ReconcileReport, PluginError>;

    /// Layer 3: load all enabled plugins into active state.
    /// Parses plugin.json, resolves dependency graph, merges contributions.
    pub fn refresh_active_plugins(
        &mut self,
        installed: &[InstalledPlugin],
    ) -> Result<ActivePluginSet, PluginError>;
}
```

## 8. Builtin Plugins

Builtin plugins use the naming convention `{name}@builtin`. They are loaded in-process with no filesystem materialization. Users can toggle them via settings (enabled by default).

```rust
pub struct BuiltinPlugin {
    pub name: String,    // e.g. "core-tools@builtin"
    pub contributions: PluginContributions,
    pub enabled: bool,   // user-toggleable via settings
}

impl PluginManager {
    /// Register builtin plugins. These always load first (before marketplace/local).
    /// No filesystem path — contributions are compiled in.
    fn load_builtins(&mut self) -> Vec<BuiltinPlugin>;
}
```

## 9. Version Management

### installed_plugins.json V1 to V2

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPluginsV2 {
    pub version: i32,  // 2
    pub plugins: Vec<InstalledPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: String,
    pub scope: PluginScope,
    pub source: PluginSource,
    pub marketplace: Option<String>,
    pub installed_at: String,     // ISO 8601
    pub updated_at: Option<String>,
}
```

V1 was a flat list of names. V2 adds scope, source, marketplace provenance, and timestamps.

### Versioned Cache Paths

Plugins are cached at `~/.coco/plugins/<name>/<version>/`. Multiple versions can coexist on disk; only the active version is loaded.

```rust
/// Calculate a deterministic version string from plugin source.
/// For git sources: short SHA. For npm/pip: package version.
/// For local/url: content hash of plugin.json.
pub fn calculate_plugin_version(source: &PluginSource, manifest: &PluginManifest) -> String;
```

## 10. Error Taxonomy

21 named error variants covering the full plugin lifecycle:

```rust
#[derive(Debug, Snafu)]
pub enum PluginError {
    // Fetch/install errors
    GitAuthFailed { url: String, source: anyhow::Error },
    GitCloneFailed { url: String, source: anyhow::Error },
    NetworkError { url: String, source: reqwest::Error },
    DownloadFailed { url: String, status: i32 },
    NpmInstallFailed { package: String, source: anyhow::Error },
    PipInstallFailed { package: String, source: anyhow::Error },
    ArchiveExtractFailed { path: PathBuf, source: anyhow::Error },

    // Manifest errors
    ManifestParseError { path: PathBuf, source: serde_json::Error },
    ManifestNotFound { path: PathBuf },
    ManifestValidationFailed { name: String, reason: String },

    // Dependency errors
    DependencyCycle { cycle: Vec<String> },
    DependencyNotFound { plugin: String, missing_dep: String },
    CrossMarketplaceDependency { plugin: String, dep: String, marketplace: String },

    // Security errors
    PathTraversal { name: String, path: PathBuf },
    Impersonation { name: String, official_pattern: String },
    BlockedByPolicy { name: String, reason: String },

    // Runtime errors
    ContributionConflict { contribution_type: String, name: String, plugins: Vec<String> },
    PluginNotFound { name: String },
    VersionMismatch { name: String, expected: String, actual: String },
    CacheDirError { path: PathBuf, source: std::io::Error },
    MigrationFailed { from_version: i32, to_version: i32, source: anyhow::Error },
}
```

## 11. Headless / CCR Mode

```rust
impl PluginManager {
    /// Install plugins for headless/CCR execution.
    /// Skips interactive prompts. Uses a zip cache mode for faster
    /// materialization (pre-packaged plugin archives instead of git clone).
    /// Falls back to normal install if cache miss.
    pub async fn install_plugins_for_headless(
        &mut self,
        settings: &Settings,
        cache_dir: &Path,
    ) -> Result<(), PluginError>;
}
```

In headless mode:
- No user prompts for approval (all managed/policy plugins auto-approved)
- Zip cache: plugins are pre-archived as `.zip` in a shared cache directory. `reconcile_marketplaces()` checks cache before network fetch.
- Timeout: stricter timeout for network operations (30s vs 120s interactive)

## Core Logic

```rust
pub struct PluginManager {
    builtins: Vec<BuiltinPlugin>,
    installed: Vec<InstalledPlugin>,
    active: Vec<LoadedPlugin>,
    dependency_graph: Vec<ResolvedPlugin>,
}

pub struct LoadedPlugin {
    pub name: String,
    pub manifest: PluginManifest,
    pub path: Option<PathBuf>,  // None for builtins
    pub source: PluginSource,
    pub scope: PluginScope,
    pub enabled: bool,
    pub is_builtin: bool,
}

impl PluginManager {
    /// Full load sequence:
    /// 1. load_builtins()
    /// 2. reconcile_marketplaces() (Layer 2)
    /// 3. refresh_active_plugins() (Layer 3)
    /// 4. resolve_transitive() dependency check
    /// 5. verify_and_demote() scope demotion
    /// 6. Merge contributions into registries
    pub async fn load(settings: &Settings) -> Result<Self, PluginError>;

    pub fn enabled(&self) -> Vec<&LoadedPlugin>;
    pub fn skills(&self) -> Vec<SkillDefinition>;
    pub fn hooks(&self) -> HooksSettings;
    pub fn mcp_servers(&self) -> HashMap<String, McpServerConfig>;
    pub fn commands(&self) -> Vec<CommandContribution>;
    pub fn agents(&self) -> Vec<AgentContribution>;
    pub fn lsp_servers(&self) -> HashMap<String, LspServerConfig>;
    pub fn output_styles(&self) -> Vec<OutputStyleContribution>;
    pub fn settings_schema(&self) -> HashMap<String, SettingSchemaEntry>;
    pub fn channels(&self) -> Vec<ChannelContribution>;
}
```
