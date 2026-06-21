//! Marketplace manager — search, install, and recommend plugins.
//!

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::schemas::KnownMarketplace;
use crate::schemas::KnownMarketplacesFile;
use crate::schemas::MarketplaceSource;
use crate::schemas::PluginAuthor;
use crate::schemas::PluginMarketplace;
use crate::schemas::PluginMarketplaceEntry;
use crate::schemas::PluginScope;
use crate::schemas::validate_marketplace_name;
use crate::schemas::validate_official_name_source;

// ---------------------------------------------------------------------------
// Official marketplace constants
// ---------------------------------------------------------------------------

/// Official marketplace name.
pub const OFFICIAL_MARKETPLACE_NAME: &str = "claude-plugins-official";

/// Official marketplace GitHub organization.
pub const OFFICIAL_GITHUB_ORG: &str = "anthropics";

/// Official marketplace source.
pub fn official_marketplace_source() -> MarketplaceSource {
    MarketplaceSource::Github {
        repo: format!("{OFFICIAL_GITHUB_ORG}/{OFFICIAL_MARKETPLACE_NAME}"),
        git_ref: None,
        path: None,
        sparse_paths: None,
    }
}

/// CDN download base URL for official marketplace.
pub const OFFICIAL_CDN_BASE: &str =
    "https://downloads.claude.ai/claude-code-releases/plugins/claude-plugins-official";

/// Names reserved for official Anthropic marketplaces.
pub const ALLOWED_OFFICIAL_MARKETPLACE_NAMES: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "life-sciences",
    "knowledge-work-plugins",
];

/// Reserved marketplace names that cannot be used by third parties.
pub const RESERVED_MARKETPLACE_NAMES: &[&str] = &["inline", "builtin"];

/// Name used for built-in plugins.
pub const BUILTIN_MARKETPLACE_NAME: &str = "builtin";

/// Check if a marketplace name belongs to the official Anthropic set.
pub fn is_official_marketplace_name(name: &str) -> bool {
    ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(&name)
}

/// Parse a fully-qualified plugin ID ("name@marketplace") into parts.
pub fn parse_plugin_id(plugin_id: &str) -> Option<(&str, &str)> {
    plugin_id.split_once('@')
}

/// Build a fully-qualified plugin ID from name and marketplace.
pub fn build_plugin_id(name: &str, marketplace: &str) -> String {
    format!("{name}@{marketplace}")
}

/// Check if a plugin ID refers to a built-in plugin.
pub fn is_builtin_plugin_id(plugin_id: &str) -> bool {
    plugin_id.ends_with("@builtin")
}

// ---------------------------------------------------------------------------
// MarketplacePlugin — public search result type
// ---------------------------------------------------------------------------

/// A plugin entry returned from marketplace search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplacePlugin {
    /// Plugin name (kebab-case).
    pub name: String,
    /// Latest available version.
    pub version: Option<String>,
    /// Brief description.
    pub description: Option<String>,
    /// Author or organization.
    pub author: Option<PluginAuthor>,
    /// Total install count (from stats endpoint, if available).
    #[serde(default)]
    pub downloads: i64,
    /// Homepage URL.
    pub homepage: Option<String>,
    /// Marketplace this plugin belongs to.
    pub marketplace: String,
    /// Tags / keywords for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl MarketplacePlugin {
    /// Build from a marketplace entry and the parent marketplace name.
    pub fn from_entry(entry: &PluginMarketplaceEntry, marketplace_name: &str) -> Self {
        Self {
            name: entry.name.clone(),
            version: entry.version.clone(),
            description: entry.description.clone(),
            author: entry.author.clone(),
            downloads: 0,
            homepage: entry.homepage.clone(),
            marketplace: marketplace_name.to_string(),
            tags: entry.tags.clone().unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// PluginRecommendation — hint system
// ---------------------------------------------------------------------------

/// A recommendation surfaced by the hint system.
///
/// Recommends a plugin when a CLI/SDK emits a `<claude-code-hint />` tag
/// referencing a plugin ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginRecommendation {
    /// Fully-qualified plugin ID ("name@marketplace").
    pub plugin_id: String,
    /// Human-readable plugin name.
    pub plugin_name: String,
    /// The marketplace that hosts the plugin.
    pub marketplace_name: String,
    /// Short description from the marketplace entry.
    pub plugin_description: Option<String>,
    /// The CLI command / SDK call that triggered the hint.
    pub source_command: String,
}

// ---------------------------------------------------------------------------
// Plugin hint recommendation pipeline
// ---------------------------------------------------------------------------

/// Hard cap on `claude_code_hints.plugin[]` — bounds config growth. Each
/// shown plugin appends one slug; past this point we stop prompting (and
/// stop appending) rather than let the config grow without limit.
pub const MAX_SHOWN_PLUGINS: usize = 100;

/// Pre-store gate called by shell tools when a `type="plugin"` hint is
/// detected. Drops the hint if:
///
///  - a dialog has already been shown this session
///  - the user has disabled hints
///  - the shown-plugins list has hit the config-growth cap
///  - the plugin slug doesn't parse as `name@marketplace`
///  - the marketplace isn't official (hardcoded for v1)
///  - the plugin was already shown in a prior session
///  - the plugin is already installed
///  - the plugin is blocked by org policy
///
/// Synchronous on purpose — shell tools shouldn't await a marketplace
/// lookup just to strip a stderr line. The async marketplace-cache check
/// happens later in [`resolve_plugin_hint`].
///
/// `installed` is the loaded installed-plugins manager (for the
/// already-installed check). When `None`, the installed check is skipped
/// (best-effort; the resolve step still gates on cache membership).
///
pub fn maybe_record_plugin_hint(
    hint: &crate::hints::ClaudeCodeHint,
    installed: Option<&crate::loader::InstalledPluginsManager>,
) {
    // Feature gate: behavior defaults on and is opt-out via the
    // persisted `disabled` flag (checked below). See followups.
    if crate::hints::has_shown_hint_this_session() {
        return;
    }

    let global = coco_config::global_config::load_global_config().unwrap_or_default();
    let state = global.claude_code_hints.unwrap_or_default();
    if state.disabled {
        return;
    }
    if state.plugin.len() >= MAX_SHOWN_PLUGINS {
        return;
    }

    let plugin_id = hint.value.as_str();
    // First '@' splits name@marketplace.
    let Some((name, marketplace)) = plugin_id.split_once('@') else {
        return;
    };
    if name.is_empty() || marketplace.is_empty() {
        return;
    }
    // Lowercase before checking the official names set.
    if !is_official_marketplace_name(&marketplace.to_lowercase()) {
        return;
    }
    if state.plugin.iter().any(|p| p == plugin_id) {
        return;
    }
    if installed.is_some_and(|m| m.is_installed(plugin_id)) {
        return;
    }
    if is_plugin_blocked_by_policy(plugin_id) {
        return;
    }

    // Bound repeat lookups on the same slug — a CLI that emits on every
    // invocation shouldn't trigger N resolve cycles for the same plugin.
    if !crate::hints::record_tried(plugin_id) {
        return;
    }

    crate::hints::set_pending_hint(hint.clone());
}

/// Whether a plugin is force-disabled by org policy (managed settings).
pub fn is_plugin_blocked_by_policy(plugin_id: &str) -> bool {
    let policy = crate::security::EnterprisePolicy::from_managed_settings();
    let id = crate::identifier::PluginId::parse(plugin_id);
    matches!(
        crate::security::check_policy(&id, /*is_user_scope*/ false, &policy),
        crate::security::PolicyVerdict::BlockedPlugin { .. }
    )
}

/// Resolve the pending hint to a renderable recommendation. Runs the
/// marketplace lookup that the sync pre-store gate skipped. Returns `None`
/// if the plugin isn't in the marketplace cache — the hint is discarded.
pub fn resolve_plugin_hint(
    hint: &crate::hints::ClaudeCodeHint,
    manager: &MarketplaceManager,
) -> Option<PluginRecommendation> {
    let plugin_id = hint.value.as_str();
    let (_, marketplace) = plugin_id.split_once('@')?;

    let (plugin, _entry) = manager.get_plugin_by_id(plugin_id)?;

    Some(PluginRecommendation {
        plugin_id: plugin_id.to_string(),
        plugin_name: plugin.name,
        marketplace_name: marketplace.to_string(),
        plugin_description: plugin.description,
        source_command: hint.source_command.clone(),
    })
}

/// Record that a prompt for this plugin was surfaced. Called regardless of
/// the user's yes/no response — show-once semantics. Best-effort: persistence
/// failures are swallowed (hint state is opportunistic).
pub fn mark_hint_plugin_shown(plugin_id: &str) {
    let mut global = match coco_config::global_config::load_global_config() {
        Ok(g) => g,
        Err(_) => return,
    };
    let state = global
        .claude_code_hints
        .get_or_insert_with(Default::default);
    if state.plugin.iter().any(|p| p == plugin_id) {
        return;
    }
    state.plugin.push(plugin_id.to_string());
    let _ = coco_config::global_config::write_global_config(&global);
}

/// Set the opt-out flag when the user picks "don't show plugin installation
/// hints again". Best-effort persistence.
pub fn disable_hint_recommendations() {
    let mut global = match coco_config::global_config::load_global_config() {
        Ok(g) => g,
        Err(_) => return,
    };
    let state = global
        .claude_code_hints
        .get_or_insert_with(Default::default);
    if state.disabled {
        return;
    }
    state.disabled = true;
    let _ = coco_config::global_config::write_global_config(&global);
}

// ---------------------------------------------------------------------------
// Install counts cache
// ---------------------------------------------------------------------------

/// Cached install-count entry fetched from the stats endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstallCountEntry {
    /// Plugin ID ("name@marketplace").
    pub plugin: String,
    pub unique_installs: i64,
}

/// On-disk cache structure for install counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallCountsCache {
    pub version: i32,
    pub fetched_at: String,
    pub counts: Vec<InstallCountEntry>,
}

impl InstallCountsCache {
    /// Load from a JSON file, returning `None` if missing or unparseable.
    pub fn load(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Persist to disk.
    pub fn save(&self, path: &Path) -> crate::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Look up the install count for a specific plugin ID.
    pub fn get_count(&self, plugin_id: &str) -> Option<i64> {
        self.counts
            .iter()
            .find(|e| e.plugin == plugin_id)
            .map(|e| e.unique_installs)
    }
}

// ---------------------------------------------------------------------------
// MarketplaceManager
// ---------------------------------------------------------------------------

/// Manages known marketplaces, caching, search, and plugin installation.
///
/// On-disk layout (under `plugins_dir`):
/// ```text
/// plugins/
///   known_marketplaces.json
///   marketplaces/
///     my-marketplace.json      (URL-sourced cache)
///     github-marketplace/      (cloned repo cache)
///       marketplace.json
///   install-counts-cache.json
/// ```
pub struct MarketplaceManager {
    /// Root plugins directory (e.g. `~/.coco/plugins/`).
    plugins_dir: PathBuf,
    /// In-memory cache of loaded marketplace manifests.
    marketplace_cache: HashMap<String, PluginMarketplace>,
}

impl MarketplaceManager {
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            marketplace_cache: HashMap::new(),
        }
    }

    /// Path to `known_marketplaces.json`.
    pub fn known_marketplaces_path(&self) -> PathBuf {
        self.plugins_dir.join("known_marketplaces.json")
    }

    /// Path to the marketplace cache directory.
    pub fn marketplace_cache_dir(&self) -> PathBuf {
        self.plugins_dir.join("marketplaces")
    }

    /// Path to the install-counts cache file.
    pub fn install_counts_cache_path(&self) -> PathBuf {
        self.plugins_dir.join("install-counts-cache.json")
    }

    // -----------------------------------------------------------------------
    // Known marketplaces I/O
    // -----------------------------------------------------------------------

    /// Load `known_marketplaces.json`, returning an empty map on any error.
    pub fn load_known_marketplaces(&self) -> KnownMarketplacesFile {
        let path = self.known_marketplaces_path();
        let Ok(content) = std::fs::read_to_string(&path) else {
            return HashMap::new();
        };
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Save `known_marketplaces.json`.
    pub fn save_known_marketplaces(&self, config: &KnownMarketplacesFile) -> crate::Result<()> {
        let path = self.known_marketplaces_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(config)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Register a new marketplace (validates name and source).
    pub fn register_marketplace(
        &mut self,
        name: &str,
        source: MarketplaceSource,
        install_location: &str,
    ) -> crate::Result<()> {
        if let Some(err) = validate_marketplace_name(name) {
            return Err(crate::PluginError::generic("marketplace", err));
        }
        if let Some(err) = validate_official_name_source(name, &source) {
            return Err(crate::PluginError::generic("marketplace", err));
        }

        let mut known = self.load_known_marketplaces();
        known.insert(
            name.to_string(),
            KnownMarketplace {
                source,
                install_location: install_location.to_string(),
                last_updated: Utc::now().to_rfc3339(),
                auto_update: None,
            },
        );
        self.save_known_marketplaces(&known)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Marketplace loading (from cache)
    // -----------------------------------------------------------------------

    /// Read-only access to a marketplace cached in memory (no fetch).
    ///
    /// Returns `None` when the marketplace hasn't been loaded via
    /// [`Self::load_cached_marketplace`] yet. Used by callers (e.g.
    /// install pipeline) that need to inspect entries / dep lists
    /// without holding a mutable borrow across `.await`.
    pub fn cached_marketplace(&self, name: &str) -> Option<&PluginMarketplace> {
        self.marketplace_cache.get(name)
    }

    /// Load a marketplace manifest from its cached location on disk.
    ///
    /// For directory-based caches, looks for `marketplace.json` or
    /// `.claude-plugin/marketplace.json` inside the directory.
    /// For file-based caches, reads the JSON file directly.
    pub fn load_cached_marketplace(&mut self, name: &str) -> crate::Result<&PluginMarketplace> {
        // Check in-memory cache first.
        if self.marketplace_cache.contains_key(name) {
            return Ok(&self.marketplace_cache[name]);
        }

        let known = self.load_known_marketplaces();
        let entry = known.get(name).ok_or_else(|| {
            crate::PluginError::generic(
                "marketplace",
                format!("marketplace '{name}' not found in known_marketplaces.json"),
            )
        })?;

        let install_loc = Path::new(&entry.install_location);
        let marketplace = read_cached_marketplace(install_loc)?;
        self.marketplace_cache.insert(name.to_string(), marketplace);
        Ok(&self.marketplace_cache[name])
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Search for plugins across all loaded marketplaces by name or keyword.
    ///
    /// Matches against plugin name, description, tags, and keywords.
    /// Case-insensitive substring matching.
    pub fn search_plugins(&self, query: &str) -> Vec<MarketplacePlugin> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for (mkt_name, marketplace) in &self.marketplace_cache {
            for entry in &marketplace.plugins {
                if matches_query(entry, &query_lower) {
                    results.push(MarketplacePlugin::from_entry(entry, mkt_name));
                }
            }
        }

        // Enrich with install counts if available.
        if let Some(cache) = InstallCountsCache::load(&self.install_counts_cache_path()) {
            for result in &mut results {
                let plugin_id = format!("{}@{}", result.name, result.marketplace);
                if let Some(count) = cache.get_count(&plugin_id) {
                    result.downloads = count;
                }
            }
        }

        // Sort by downloads descending, then name ascending.
        results.sort_by(|a, b| {
            b.downloads
                .cmp(&a.downloads)
                .then_with(|| a.name.cmp(&b.name))
        });

        results
    }

    /// List all plugins across all loaded marketplaces.
    pub fn list_all_plugins(&self) -> Vec<MarketplacePlugin> {
        self.search_plugins("")
    }

    /// Look up a specific plugin by its ID ("name@marketplace").
    pub fn get_plugin_by_id(
        &self,
        plugin_id: &str,
    ) -> Option<(MarketplacePlugin, &PluginMarketplaceEntry)> {
        let (name, mkt) = plugin_id.split_once('@')?;
        let marketplace = self.marketplace_cache.get(mkt)?;
        let entry = marketplace.plugins.iter().find(|e| e.name == name)?;
        Some((MarketplacePlugin::from_entry(entry, mkt), entry))
    }

    // -----------------------------------------------------------------------
    // Install
    // -----------------------------------------------------------------------

    /// Install a plugin from a marketplace to a local cache directory.
    ///
    /// For local/relative-path sources, copies the plugin directory.
    /// Returns the path where the plugin was installed.
    pub async fn install_plugin(
        &self,
        marketplace_name: &str,
        entry: &PluginMarketplaceEntry,
        scope: PluginScope,
    ) -> crate::Result<PathBuf> {
        let known = self.load_known_marketplaces();
        let mkt_entry = known.get(marketplace_name).ok_or_else(|| {
            crate::PluginError::generic(
                "marketplace",
                format!("marketplace '{marketplace_name}' is not registered"),
            )
        })?;

        let cache_dir = self
            .plugins_dir
            .join("cache")
            .join(sanitize_for_path(marketplace_name))
            .join(sanitize_for_path(&entry.name));

        let version_dir = match &entry.version {
            Some(v) => cache_dir.join(sanitize_for_path(v)),
            None => cache_dir.join("latest"),
        };

        std::fs::create_dir_all(&version_dir)?;

        // For local sources, copy plugin content.
        match &entry.source {
            crate::schemas::PluginSource::RelativePath(rel_path) => {
                let mkt_base = Path::new(&mkt_entry.install_location);
                let source_dir = if mkt_base.is_dir() {
                    mkt_base.join(rel_path)
                } else {
                    // File-based marketplace: resolve relative to parent dir.
                    mkt_base.parent().unwrap_or(mkt_base).join(rel_path)
                };

                if !source_dir.is_dir() {
                    return Err(crate::PluginError::generic(
                        "marketplace",
                        format!(
                            "plugin source directory not found: {}",
                            source_dir.display()
                        ),
                    ));
                }

                copy_dir_contents(&source_dir, &version_dir)?;
            }
            crate::schemas::PluginSource::Remote(remote) => {
                // Remote plugin source: the manifest points at an external
                // repo / registry (not a subdir of the marketplace). Fetch it
                // per-plugin (git clone / npm / pip). On any failure remove the
                // empty version dir so no broken enabled-but-empty entry is
                // left on disk for the loader to choke on.
                if let Err(e) = crate::fetch::fetch_plugin_source(remote, &version_dir).await {
                    let _ = std::fs::remove_dir_all(&version_dir);
                    return Err(e);
                }
            }
        }

        tracing::info!(
            plugin = %entry.name,
            version = ?entry.version,
            scope = ?scope,
            path = %version_dir.display(),
            "plugin installed to cache"
        );

        Ok(version_dir)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a cached marketplace manifest from disk.
///
/// Handles both file (`.json`) and directory (with inner `marketplace.json`
/// or `.claude-plugin/marketplace.json`) formats.
fn read_cached_marketplace(path: &Path) -> crate::Result<PluginMarketplace> {
    let json_path = if path.is_dir() {
        let direct = path.join("marketplace.json");
        if direct.exists() {
            direct
        } else {
            let nested = path.join(".claude-plugin").join("marketplace.json");
            if nested.exists() {
                nested
            } else {
                return Err(crate::PluginError::generic(
                    "marketplace",
                    format!("no marketplace.json found in {}", path.display()),
                ));
            }
        }
    } else {
        path.to_path_buf()
    };

    let content = std::fs::read_to_string(&json_path)?;
    let marketplace: PluginMarketplace = serde_json::from_str(&content)?;
    Ok(marketplace)
}

/// Check if a marketplace entry matches a search query.
fn matches_query(entry: &PluginMarketplaceEntry, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }

    if entry.name.to_lowercase().contains(query_lower) {
        return true;
    }
    if let Some(ref desc) = entry.description
        && desc.to_lowercase().contains(query_lower)
    {
        return true;
    }
    if let Some(ref tags) = entry.tags
        && tags.iter().any(|t| t.to_lowercase().contains(query_lower))
    {
        return true;
    }
    if let Some(ref kw) = entry.keywords
        && kw.iter().any(|k| k.to_lowercase().contains(query_lower))
    {
        return true;
    }
    false
}

/// Sanitize a string for use as a filesystem path segment.
fn sanitize_for_path(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            result.push(c);
        } else {
            result.push('-');
        }
    }
    result
}

/// Recursively copy directory contents (files only, shallow).
fn copy_dir_contents(src: &Path, dst: &Path) -> crate::Result<()> {
    if !src.is_dir() {
        return Err(crate::PluginError::generic(
            "marketplace",
            format!("source is not a directory: {}", src.display()),
        ));
    }
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin delisting detection
// ---------------------------------------------------------------------------

/// Detect plugins installed from a marketplace that are no longer listed there.
///
/// Returns plugin IDs in `"name@marketplace"` format that have been delisted.
pub fn detect_delisted_plugins(
    installed: &crate::loader::InstalledPluginsManager,
    marketplace: &PluginMarketplace,
    marketplace_name: &str,
) -> Vec<String> {
    let listed_names: std::collections::HashSet<&str> = marketplace
        .plugins
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    let suffix = format!("@{marketplace_name}");

    installed
        .installed_plugin_ids()
        .into_iter()
        .filter(|id| {
            id.ends_with(&suffix) && {
                let plugin_name = &id[..id.len() - suffix.len()];
                !listed_names.contains(plugin_name)
            }
        })
        .map(String::from)
        .collect()
}

/// Record of a delisted plugin for flagging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlaggedPlugin {
    pub plugin_id: String,
    pub flagged_at: String,
    pub marketplace: String,
}

/// Load flagged plugins from disk.
pub fn load_flagged_plugins(plugins_dir: &Path) -> Vec<FlaggedPlugin> {
    let path = plugins_dir.join("flagged_plugins.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Save flagged plugins to disk.
pub fn save_flagged_plugins(plugins_dir: &Path, flagged: &[FlaggedPlugin]) -> crate::Result<()> {
    let path = plugins_dir.join("flagged_plugins.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(flagged)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Flag a delisted plugin.
pub fn flag_delisted_plugin(
    plugins_dir: &Path,
    plugin_id: &str,
    marketplace_name: &str,
) -> crate::Result<()> {
    let mut flagged = load_flagged_plugins(plugins_dir);
    if flagged.iter().any(|f| f.plugin_id == plugin_id) {
        return Ok(());
    }
    flagged.push(FlaggedPlugin {
        plugin_id: plugin_id.to_string(),
        flagged_at: Utc::now().to_rfc3339(),
        marketplace: marketplace_name.to_string(),
    });
    save_flagged_plugins(plugins_dir, &flagged)
}

/// Startup delisting sweep: uninstall every plugin installed from a known
/// marketplace that is no longer listed in that marketplace's current manifest.
///
/// For each known marketplace it diffs the installed ledger against the cached
/// manifest, flags newly-delisted plugins (`flagged_plugins.json`), removes them
/// from the installed ledger, and persists. A marketplace whose manifest can't be
/// read is skipped (a fetch failure must never nuke installed plugins).
/// `config_home` is the coco config root; the plugins dir is
/// `<config_home>/plugins`. Returns the uninstalled plugin ids (`name@marketplace`).
pub fn detect_and_uninstall_delisted_plugins(config_home: &Path) -> Vec<String> {
    let plugins_dir = config_home.join("plugins");
    let installed_path = plugins_dir.join("installed_plugins.json");
    let Ok(mut installed) = crate::loader::InstalledPluginsManager::load(installed_path) else {
        return Vec::new();
    };
    if installed.installed_plugin_ids().is_empty() {
        return Vec::new();
    }

    let mut mgr = MarketplaceManager::new(plugins_dir.clone());
    let names: Vec<String> = mgr.load_known_marketplaces().into_keys().collect();

    let mut delisted_all: Vec<String> = Vec::new();
    for name in &names {
        // A marketplace we can't load the manifest for is skipped — never treat
        // an unreadable/uncached manifest as "everything delisted".
        let Ok(marketplace) = mgr.load_cached_marketplace(name) else {
            continue;
        };
        let delisted = detect_delisted_plugins(&installed, marketplace, name);
        for id in delisted {
            // Audit trail before removal (idempotent).
            let _ = flag_delisted_plugin(&plugins_dir, &id, name);
            installed.remove_plugin(&id);
            delisted_all.push(id);
        }
    }

    if !delisted_all.is_empty()
        && let Err(e) = installed.save()
    {
        tracing::warn!(
            target: "coco::plugins",
            error = %e,
            "failed to persist installed ledger after delisting sweep"
        );
    }
    delisted_all
}

// ---------------------------------------------------------------------------
// Plugin auto-update
// ---------------------------------------------------------------------------

/// Auto-update check result for a single plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoUpdateCheck {
    pub plugin_id: String,
    pub current_version: Option<String>,
    pub available_version: Option<String>,
    pub needs_update: bool,
}

// ---------------------------------------------------------------------------
// Seed marketplaces + reconcile-on-startup
// ---------------------------------------------------------------------------

/// Read-only plugin seed directories from `COCO_PLUGIN_SEED_DIR`
/// (PATH-delimited, precedence order). Empty when unset. Seed dirs are expected
/// to be absolute (the container-image use case); no tilde expansion.
pub fn get_plugin_seed_dirs() -> Vec<PathBuf> {
    let Some(raw) = coco_config::env::env_opt(coco_config::EnvKey::CocoPluginSeedDir) else {
        return Vec::new();
    };
    std::env::split_paths(&raw)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn read_seed_known_marketplaces(seed_dir: &Path) -> Option<KnownMarketplacesFile> {
    let content = std::fs::read_to_string(seed_dir.join("known_marketplaces.json")).ok()?;
    serde_json::from_str(&content).ok()
}

/// Resolve a seed marketplace's on-disk location relative to THIS seed dir
/// (not the build-time path baked into the seed JSON). Returns the first of
/// `<seed>/marketplaces/<name>` (dir) or `<seed>/marketplaces/<name>.json` that
/// exists.
fn find_seed_marketplace_location(seed_dir: &Path, name: &str) -> Option<PathBuf> {
    let base = seed_dir.join("marketplaces");
    let candidates = [base.join(name), base.join(format!("{name}.json"))];
    candidates.into_iter().find(|c| c.exists())
}

/// Register seed marketplaces (`COCO_PLUGIN_SEED_DIR`) into the primary
/// `known_marketplaces.json`. Idempotent; first-seed-wins across multiple seed
/// dirs; seed entries win over the primary (admin-managed, baked into the image).
/// `install_location` is recomputed from the runtime seed dir; `auto_update` is
/// forced off (seed is read-only). Returns true if anything changed (caller
/// should clear caches).
pub fn register_seed_marketplaces(plugins_dir: &Path) -> bool {
    register_seed_marketplaces_from(plugins_dir, &get_plugin_seed_dirs())
}

/// Env-free core of [`register_seed_marketplaces`] — takes the seed dirs
/// explicitly so it's testable without mutating the process environment.
fn register_seed_marketplaces_from(plugins_dir: &Path, seed_dirs: &[PathBuf]) -> bool {
    if seed_dirs.is_empty() {
        return false;
    }
    let mgr = MarketplaceManager::new(plugins_dir.to_path_buf());
    let mut primary = mgr.load_known_marketplaces();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut changed = 0usize;

    for seed_dir in seed_dirs {
        let Some(seed_cfg) = read_seed_known_marketplaces(seed_dir) else {
            continue;
        };
        for (name, seed_entry) in seed_cfg {
            if claimed.contains(&name) {
                continue;
            }
            let Some(loc) = find_seed_marketplace_location(seed_dir, &name) else {
                // Content missing (incomplete image) — don't claim the name; a
                // later seed dir may carry working content.
                tracing::warn!(
                    target: "coco::plugins",
                    seed = %seed_dir.display(),
                    marketplace = %name,
                    "seed marketplace content missing; skipping"
                );
                continue;
            };
            claimed.insert(name.clone());
            let desired = KnownMarketplace {
                source: seed_entry.source,
                install_location: loc.to_string_lossy().into_owned(),
                last_updated: seed_entry.last_updated,
                auto_update: Some(false),
            };
            if primary.get(&name) == Some(&desired) {
                continue; // idempotent no-op
            }
            primary.insert(name, desired); // seed wins
            changed += 1;
        }
    }

    if changed > 0 {
        if let Err(e) = mgr.save_known_marketplaces(&primary) {
            tracing::warn!(target: "coco::plugins", error = %e, "failed to save known_marketplaces after seed sync");
            return false;
        }
        tracing::info!(target: "coco::plugins", changed, "synced marketplaces from seed dir(s)");
        return true;
    }
    false
}

/// User-declared marketplaces from settings.json `extraKnownMarketplaces`
/// (name → source). The implicit official marketplace is intentionally NOT
/// included here — it is owned by [`crate::official::ensure_official_marketplace`]
/// (retry/backoff-gated).
pub fn get_declared_marketplaces(config_home: &Path) -> HashMap<String, MarketplaceSource> {
    let mut out = HashMap::new();
    let Ok(raw) = std::fs::read_to_string(config_home.join("settings.json")) else {
        return out;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    let Some(obj) = value
        .get("extra_known_marketplaces")
        .or_else(|| value.get("extraKnownMarketplaces"))
        .and_then(serde_json::Value::as_object)
    else {
        return out;
    };
    for (name, entry) in obj {
        // DeclaredMarketplace = `{ source: MarketplaceSource, ... }`.
        if let Some(src) = entry.get("source")
            && let Ok(source) = serde_json::from_value::<MarketplaceSource>(src.clone())
        {
            out.insert(name.clone(), source);
        }
    }
    out
}

/// Reconcile declared (settings `extraKnownMarketplaces`) marketplaces against
/// materialized state: fetch + register any declared marketplace not present in
/// `known_marketplaces.json`, or whose source changed. Best-effort, idempotent,
/// additive — a fetch failure logs + skips (never aborts the rest). Returns the
/// names installed/updated. The implicit official marketplace is handled
/// separately.
pub async fn reconcile_marketplaces(plugins_dir: &Path, config_home: &Path) -> Vec<String> {
    let declared = get_declared_marketplaces(config_home);
    if declared.is_empty() {
        return Vec::new();
    }
    let mut mgr = MarketplaceManager::new(plugins_dir.to_path_buf());
    let known = mgr.load_known_marketplaces();
    let cache_dir = mgr.marketplace_cache_dir();
    let mut done = Vec::new();
    for (name, source) in declared {
        let needs = match known.get(&name) {
            None => true,                          // missing → install
            Some(entry) => entry.source != source, // source changed → update
        };
        if !needs {
            continue;
        }
        match crate::fetch::fetch_marketplace(&source, &name, &cache_dir).await {
            Ok(loc) => match mgr.register_marketplace(&name, source, &loc.to_string_lossy()) {
                Ok(()) => done.push(name),
                Err(e) => {
                    tracing::warn!(target: "coco::plugins", marketplace = %name, error = %e, "reconcile: register failed")
                }
            },
            Err(e) => {
                tracing::warn!(target: "coco::plugins", marketplace = %name, error = %e, "reconcile: fetch failed; skipping")
            }
        }
    }
    done
}

/// Startup marketplace maintenance: register seed marketplaces, reconcile
/// declared marketplaces, then uninstall delisted plugins. Best-effort +
/// idempotent — safe to call fire-and-forget after
/// [`crate::official::ensure_official_marketplace`].
/// Returns the delisted plugin ids (for logging).
pub async fn run_marketplace_startup(config_home: &Path) -> Vec<String> {
    let plugins_dir = config_home.join("plugins");
    register_seed_marketplaces(&plugins_dir);
    let _reconciled = reconcile_marketplaces(&plugins_dir, config_home).await;
    detect_and_uninstall_delisted_plugins(config_home)
}

/// Official marketplace names that should NOT auto-update by default.
const NO_AUTO_UPDATE_OFFICIAL: &[&str] = &["knowledge-work-plugins"];

/// Check if auto-update is enabled for a marketplace.
pub fn is_marketplace_auto_update(marketplace_name: &str, explicit_setting: Option<bool>) -> bool {
    if let Some(setting) = explicit_setting {
        return setting;
    }
    let lower = marketplace_name.to_lowercase();
    is_official_marketplace_name(&lower) && !NO_AUTO_UPDATE_OFFICIAL.contains(&lower.as_str())
}

/// Check which installed plugins need updating.
///
/// Compares installed versions against marketplace entries.
pub fn check_plugin_updates(
    installed: &crate::loader::InstalledPluginsManager,
    marketplace: &PluginMarketplace,
    marketplace_name: &str,
) -> Vec<AutoUpdateCheck> {
    let mut checks = Vec::new();

    for entry in &marketplace.plugins {
        let plugin_id = format!("{}@{marketplace_name}", entry.name);
        if !installed.is_installed(&plugin_id) {
            continue;
        }

        let installations = installed.get_installations(&plugin_id);
        let current_version = installations
            .iter()
            .filter_map(|i| i.version.as_deref())
            .next()
            .map(String::from);

        let available_version = entry.version.clone();
        let needs_update = match (&current_version, &available_version) {
            (Some(current), Some(available)) => current != available,
            (None, Some(_)) => true,
            _ => false,
        };

        checks.push(AutoUpdateCheck {
            plugin_id,
            current_version,
            available_version,
            needs_update,
        });
    }

    checks
}

#[cfg(test)]
#[path = "marketplace.test.rs"]
mod tests;
