//! Plugin schema types — manifest, marketplace, installation records.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde::Serialize;

/// Regex for detecting official marketplace name impersonation.
static BLOCKED_OFFICIAL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:official[^a-z0-9]*(anthropic|claude)|(?:anthropic|claude)[^a-z0-9]*official|^(?:anthropic|claude)[^a-z0-9]*(marketplace|plugins|official))"
    ).unwrap_or_else(|e| unreachable!("static regex failed to compile: {e}"))
});

/// Regex for validating user_config option keys (valid identifiers).
static USER_CONFIG_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z_]\w*$")
        .unwrap_or_else(|e| unreachable!("static regex failed to compile: {e}"))
});

/// Regex for validating environment variable names (UPPER_SNAKE_CASE).
static ENV_VAR_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Z_][A-Z0-9_]*$")
        .unwrap_or_else(|e| unreachable!("static regex failed to compile: {e}"))
});

// ---------------------------------------------------------------------------
// Plugin author
// ---------------------------------------------------------------------------

/// Author or organization information for a plugin or marketplace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginAuthor {
    /// Display name of the author or organization.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Plugin manifest (V2 — full manifest from plugin.json / PLUGIN.toml)
// ---------------------------------------------------------------------------

/// Full plugin manifest.
///
/// Loaded from `PLUGIN.toml` (Rust convention) or `plugin.json` (JSON compat).
/// Unknown top-level fields are silently ignored via `#[serde(deny_unknown_fields)]`
/// being intentionally absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifestV2 {
    /// Unique plugin identifier (kebab-case, no spaces).
    pub name: String,

    /// Semantic version (e.g. "1.2.3").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Brief user-facing description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Author information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,

    /// Plugin homepage URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// Source code repository URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// SPDX license identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Tags for discovery and categorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,

    /// Dependency plugin references (bare name or "name@marketplace").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,

    // -- contribution paths ---
    /// Extra skill directory paths (relative to plugin root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<ManifestPaths>,

    /// Hook definitions -- inline object, file path, or array of either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<ManifestHooks>,

    /// Extra agent markdown file paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<ManifestPaths>,

    /// Extra command file/dir paths or object mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<ManifestCommands>,

    /// MCP server configurations (inline or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<serde_json::Value>,

    /// LSP server configurations (inline or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsp_servers: Option<serde_json::Value>,

    /// Output style paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_styles: Option<ManifestPaths>,

    /// Channel declarations (MCP servers as message channels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<PluginChannel>>,

    /// User-configurable options prompted at enable time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_config: Option<HashMap<String, UserConfigOption>>,

    /// Settings to merge when the plugin is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<HashMap<String, serde_json::Value>>,

    /// Environment variable requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_vars: Option<Vec<EnvVarDeclaration>>,

    /// Minimum required host version (semver, e.g. "1.2.0").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,

    /// Maximum supported host version (semver, e.g. "2.0.0").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_version: Option<String>,
}

/// Either a single path or a list of paths.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ManifestPaths {
    Single(String),
    Multiple(Vec<String>),
}

// ---------------------------------------------------------------------------
// Commands contribution types
// ---------------------------------------------------------------------------

/// Command metadata from manifest object-mapping format.
///
/// Either `source` (file path) or `content` (inline markdown), but not both.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandMetadata {
    /// Relative path to command markdown file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Inline markdown content (mutually exclusive with source).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Description override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Argument hint override (e.g., "[file]").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Typed argument shape for completion behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_kind: Option<coco_types::CommandArgumentKind>,
    /// Default model for this command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tools allowed when command runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
}

/// Commands contribution in plugin manifest.
///
/// Supports path, array, or object mapping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ManifestCommands {
    /// Single path to command file or directory.
    SinglePath(String),
    /// Array of paths to command files or directories.
    MultiplePaths(Vec<String>),
    /// Object mapping command names to metadata.
    ObjectMapping(HashMap<String, CommandMetadata>),
}

// ---------------------------------------------------------------------------
// Hooks contribution types
// ---------------------------------------------------------------------------

/// Hooks contribution in plugin manifest.
///
/// File path, inline config, or array of either.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ManifestHooks {
    /// Path to a hooks JSON file (relative to plugin root).
    FilePath(String),
    /// Inline hooks configuration.
    Inline(HashMap<String, serde_json::Value>),
    /// Array of file paths and/or inline configs.
    Multiple(Vec<ManifestHooksEntry>),
}

/// A single entry in a `ManifestHooks::Multiple` array.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ManifestHooksEntry {
    /// Path to a hooks JSON file.
    FilePath(String),
    /// Inline hooks configuration.
    Inline(HashMap<String, serde_json::Value>),
}

impl ManifestPaths {
    /// Flatten to a vec of paths.
    pub fn to_vec(&self) -> Vec<&str> {
        match self {
            ManifestPaths::Single(s) => vec![s.as_str()],
            ManifestPaths::Multiple(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// A channel declaration in the plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginChannel {
    /// Name of the MCP server this channel binds to.
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_config: Option<HashMap<String, UserConfigOption>>,
}

/// A user-configurable option for a plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserConfigOption {
    /// Value type (string, number, boolean, directory, file).
    #[serde(rename = "type")]
    pub config_type: UserConfigType,
    /// Human-readable label.
    pub title: String,
    /// Help text.
    pub description: String,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub multiple: Option<bool>,
    #[serde(default)]
    pub sensitive: Option<bool>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

/// Supported config value types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserConfigType {
    String,
    Number,
    Boolean,
    Directory,
    File,
}

/// An environment variable declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvVarDeclaration {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

// ---------------------------------------------------------------------------
// Plugin ID
// ---------------------------------------------------------------------------

/// A validated plugin identifier in "name@marketplace" format.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginId {
    pub name: String,
    pub marketplace: String,
}

impl PluginId {
    /// Parse a "name@marketplace" string, returning `None` if invalid.
    pub fn parse(s: &str) -> Option<Self> {
        let (name, marketplace) = s.split_once('@')?;
        if name.is_empty() || marketplace.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            marketplace: marketplace.to_string(),
        })
    }

    /// Format as "name@marketplace".
    pub fn as_str(&self) -> String {
        format!("{}@{}", self.name, self.marketplace)
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.marketplace)
    }
}

impl Serialize for PluginId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        PluginId::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid plugin ID: expected 'name@marketplace', got '{s}'"
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Marketplace source
// ---------------------------------------------------------------------------

/// Where a marketplace can be fetched from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum MarketplaceSource {
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
    },
    Github {
        repo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sparse_paths: Option<Vec<String>>,
    },
    Git {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sparse_paths: Option<Vec<String>>,
    },
    Npm {
        package: String,
    },
    File {
        path: String,
    },
    Directory {
        path: String,
    },
}

/// Whether a marketplace source points at a local filesystem path.
pub fn is_local_marketplace_source(source: &MarketplaceSource) -> bool {
    matches!(
        source,
        MarketplaceSource::File { .. } | MarketplaceSource::Directory { .. }
    )
}

// ---------------------------------------------------------------------------
// Plugin source (where an individual plugin is fetched from)
// ---------------------------------------------------------------------------

/// Where an individual plugin can be fetched from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PluginSource {
    /// Relative path within the marketplace directory.
    RelativePath(String),
    /// A structured remote source.
    Remote(RemotePluginSource),
}

/// Remote plugin source variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RemotePluginSource {
    Npm {
        package: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        registry: Option<String>,
    },
    Pip {
        package: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        registry: Option<String>,
    },
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    Github {
        repo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    #[serde(rename = "git-subdir")]
    GitSubdir {
        url: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
}

/// Check if a plugin source is a local relative path (starts with `./`).
pub fn is_local_plugin_source(source: &PluginSource) -> bool {
    matches!(source, PluginSource::RelativePath(p) if p.starts_with("./"))
}

// ---------------------------------------------------------------------------
// Marketplace entry (a plugin listed in marketplace.json)
// ---------------------------------------------------------------------------

/// A single plugin entry within a marketplace manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginMarketplaceEntry {
    pub name: String,
    pub source: PluginSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// When true (default), `plugin.json` is required in the plugin dir.
    #[serde(default = "default_true")]
    pub strict: bool,
    // Inherits optional manifest fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Marketplace manifest (marketplace.json)
// ---------------------------------------------------------------------------

/// A curated collection of plugins.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginMarketplace {
    pub name: String,
    pub owner: PluginAuthor,
    pub plugins: Vec<PluginMarketplaceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_remove_deleted_plugins: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MarketplaceMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_cross_marketplace_dependencies_on: Option<Vec<String>>,
}

/// Optional marketplace metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Known marketplace tracking (known_marketplaces.json)
// ---------------------------------------------------------------------------

/// Entry in the known_marketplaces.json file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnownMarketplace {
    pub source: MarketplaceSource,
    pub install_location: String,
    pub last_updated: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
}

/// The known_marketplaces.json file maps marketplace names to their metadata.
pub type KnownMarketplacesFile = HashMap<String, KnownMarketplace>;

// ---------------------------------------------------------------------------
// Plugin installation records
// ---------------------------------------------------------------------------

/// Plugin installation scope.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginScope {
    Managed,
    User,
    Project,
    Local,
}

/// A single installation entry for a plugin (V2 format).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginInstallationEntry {
    pub scope: PluginScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub install_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_sha: Option<String>,
}

/// Installed plugin metadata (V1 format, keyed by plugin ID).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledPluginV1 {
    pub version: String,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    pub install_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_sha: Option<String>,
}

/// The installed_plugins.json V1 format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledPluginsFileV1 {
    pub version: i32,
    pub plugins: HashMap<String, InstalledPluginV1>,
}

/// The installed_plugins.json V2 format (multi-scope).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledPluginsFileV2 {
    pub version: i32,
    pub plugins: HashMap<String, Vec<PluginInstallationEntry>>,
}

impl Default for InstalledPluginsFileV2 {
    fn default() -> Self {
        Self {
            version: 2,
            plugins: HashMap::new(),
        }
    }
}

/// Convenience record for tracking a single plugin installation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginInstallationRecord {
    /// Plugin name (kebab-case).
    pub name: String,
    /// Installed version string.
    pub version: String,
    /// ISO 8601 timestamp of installation.
    pub installed_at: String,
    /// Source URL or identifier where the plugin was fetched from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Installation scope.
    pub scope: PluginScope,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Errors from manifest validation.
#[derive(Debug, Clone)]
pub struct ManifestValidationError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ManifestValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// Official marketplace names reserved for Anthropic.
const ALLOWED_OFFICIAL_NAMES: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "life-sciences",
    "knowledge-work-plugins",
];

/// Check whether a marketplace name is a reserved official name.
pub fn is_allowed_official_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ALLOWED_OFFICIAL_NAMES.iter().any(|n| *n == lower)
}

/// Check whether a marketplace name impersonates an official marketplace.
pub fn is_blocked_official_name(name: &str) -> bool {
    if is_allowed_official_name(name) {
        return false;
    }
    // Non-ASCII → homograph attack vector.
    if name.bytes().any(|b| !(0x20..=0x7E).contains(&b)) {
        return true;
    }
    BLOCKED_OFFICIAL_PATTERN.is_match(name)
}

/// Validate a PluginManifestV2, returning a list of issues (empty = valid).
pub fn validate_manifest(manifest: &PluginManifestV2) -> Vec<ManifestValidationError> {
    let mut errors = Vec::new();

    if manifest.name.is_empty() {
        errors.push(ManifestValidationError {
            field: "name".to_string(),
            message: "Plugin name cannot be empty".to_string(),
        });
    }
    if manifest.name.contains(' ') {
        errors.push(ManifestValidationError {
            field: "name".to_string(),
            message: "Plugin name cannot contain spaces. Use kebab-case".to_string(),
        });
    }
    if manifest.name.contains('/') || manifest.name.contains('\\') || manifest.name.contains("..") {
        errors.push(ManifestValidationError {
            field: "name".to_string(),
            message: "Plugin name cannot contain path separators or '..'".to_string(),
        });
    }

    if let Some(ref version) = manifest.version
        && version.is_empty()
    {
        errors.push(ManifestValidationError {
            field: "version".to_string(),
            message: "Version string cannot be empty when present".to_string(),
        });
    }

    if let Some(ref homepage) = manifest.homepage
        && !homepage.starts_with("http://")
        && !homepage.starts_with("https://")
    {
        errors.push(ManifestValidationError {
            field: "homepage".to_string(),
            message: "Homepage must be a valid URL".to_string(),
        });
    }

    if let Some(ref deps) = manifest.dependencies {
        for (i, dep) in deps.iter().enumerate() {
            if dep.is_empty() {
                errors.push(ManifestValidationError {
                    field: format!("dependencies[{i}]"),
                    message: "Dependency reference cannot be empty".to_string(),
                });
            }
        }
    }

    if let Some(ref user_config) = manifest.user_config {
        for key in user_config.keys() {
            if !USER_CONFIG_KEY_RE.is_match(key) {
                errors.push(ManifestValidationError {
                    field: format!("user_config.{key}"),
                    message: "Option keys must be valid identifiers".to_string(),
                });
            }
        }
    }

    if let Some(ref min_ver) = manifest.min_version {
        errors.extend(validate_semver_field("min_version", min_ver));
    }
    if let Some(ref max_ver) = manifest.max_version {
        errors.extend(validate_semver_field("max_version", max_ver));
    }

    if let Some(ref env_vars) = manifest.env_vars {
        errors.extend(validate_env_var_declarations(env_vars));
    }

    if let Some(ref author) = manifest.author {
        errors.extend(validate_author(author));
    }

    errors
}

/// Validate a semver-like version field (major.minor.patch, each numeric).
fn validate_semver_field(field: &str, value: &str) -> Vec<ManifestValidationError> {
    let mut errors = Vec::new();
    if value.is_empty() {
        errors.push(ManifestValidationError {
            field: field.to_string(),
            message: "Version string cannot be empty".to_string(),
        });
        return errors;
    }
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.parse::<u32>().is_err()) {
        errors.push(ManifestValidationError {
            field: field.to_string(),
            message: format!("Expected semver format (MAJOR.MINOR.PATCH), got '{value}'"),
        });
    }
    errors
}

/// Validate a `PluginAuthor`.
pub fn validate_author(author: &PluginAuthor) -> Vec<ManifestValidationError> {
    let mut errors = Vec::new();
    if author.name.is_empty() {
        errors.push(ManifestValidationError {
            field: "author.name".to_string(),
            message: "Author name cannot be empty".to_string(),
        });
    }
    if let Some(ref url) = author.url
        && !url.starts_with("http://")
        && !url.starts_with("https://")
    {
        errors.push(ManifestValidationError {
            field: "author.url".to_string(),
            message: "Author URL must be a valid HTTP(S) URL".to_string(),
        });
    }
    errors
}

/// Validate environment variable declarations.
pub fn validate_env_var_declarations(
    env_vars: &[EnvVarDeclaration],
) -> Vec<ManifestValidationError> {
    let mut errors = Vec::new();
    for (i, decl) in env_vars.iter().enumerate() {
        if decl.name.is_empty() {
            errors.push(ManifestValidationError {
                field: format!("env_vars[{i}].name"),
                message: "Environment variable name cannot be empty".to_string(),
            });
        } else if !ENV_VAR_NAME_RE.is_match(&decl.name) {
            errors.push(ManifestValidationError {
                field: format!("env_vars[{i}].name"),
                message: format!(
                    "Environment variable name '{}' must be uppercase with underscores (e.g. MY_API_KEY)",
                    decl.name
                ),
            });
        }
    }
    errors
}

/// Validate a marketplace entry, returning a list of issues.
pub fn validate_marketplace_entry(entry: &PluginMarketplaceEntry) -> Vec<ManifestValidationError> {
    let mut errors = Vec::new();
    if entry.name.is_empty() {
        errors.push(ManifestValidationError {
            field: "name".to_string(),
            message: "Marketplace entry plugin name cannot be empty".to_string(),
        });
    }
    if entry.name.contains(' ') {
        errors.push(ManifestValidationError {
            field: "name".to_string(),
            message: "Plugin name cannot contain spaces".to_string(),
        });
    }
    if let Some(ref version) = entry.version
        && version.is_empty()
    {
        errors.push(ManifestValidationError {
            field: "version".to_string(),
            message: "Version string cannot be empty when present".to_string(),
        });
    }
    if let Some(ref deps) = entry.dependencies {
        for (i, dep) in deps.iter().enumerate() {
            if dep.is_empty() {
                errors.push(ManifestValidationError {
                    field: format!("dependencies[{i}]"),
                    message: "Dependency reference cannot be empty".to_string(),
                });
            }
        }
    }
    errors
}

/// Validate a marketplace name, returning an error message or None.
pub fn validate_marketplace_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Marketplace must have a name".to_string());
    }
    if name.contains(' ') {
        return Some("Marketplace name cannot contain spaces. Use kebab-case".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name == "." {
        return Some("Marketplace name cannot contain path separators or '..'".to_string());
    }
    if is_blocked_official_name(name) {
        return Some(
            "Marketplace name impersonates an official Anthropic/Claude marketplace".to_string(),
        );
    }
    if name.eq_ignore_ascii_case("inline") {
        return Some("Marketplace name 'inline' is reserved for session plugins".to_string());
    }
    if name.eq_ignore_ascii_case("builtin") {
        return Some("Marketplace name 'builtin' is reserved for built-in plugins".to_string());
    }
    None
}

/// Validate that a reserved official name comes from the official Anthropic source.
pub fn validate_official_name_source(name: &str, source: &MarketplaceSource) -> Option<String> {
    let lower = name.to_lowercase();
    if !ALLOWED_OFFICIAL_NAMES.iter().any(|n| *n == lower) {
        return None;
    }

    const OFFICIAL_ORG: &str = "anthropics";

    match source {
        MarketplaceSource::Github { repo, .. } => {
            if repo.to_lowercase().starts_with(&format!("{OFFICIAL_ORG}/")) {
                None
            } else {
                Some(format!(
                    "The name '{name}' is reserved for official Anthropic marketplaces. \
                     Only repos from 'github.com/{OFFICIAL_ORG}/' can use this name."
                ))
            }
        }
        MarketplaceSource::Git { url, .. } => {
            let lower_url = url.to_lowercase();
            if lower_url.contains(&format!("github.com/{OFFICIAL_ORG}/"))
                || lower_url.contains(&format!("git@github.com:{OFFICIAL_ORG}/"))
            {
                None
            } else {
                Some(format!(
                    "The name '{name}' is reserved for official Anthropic marketplaces. \
                     Only repos from 'github.com/{OFFICIAL_ORG}/' can use this name."
                ))
            }
        }
        _ => Some(format!(
            "The name '{name}' is reserved for official Anthropic marketplaces \
             and can only be used with GitHub sources from the '{OFFICIAL_ORG}' organization."
        )),
    }
}

#[cfg(test)]
#[path = "schemas.test.rs"]
mod tests;
