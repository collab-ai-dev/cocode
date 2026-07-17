# coco-config — Crate Plan

TS source: `utils/settings/`, `utils/model/`, `utils/effort.ts`, `utils/fastMode.ts`, `constants/`, `migrations/`, `utils/envUtils.ts`, `utils/config.ts`

## Scope

coco-config owns ONLY:
- `~/.coco.json` (GlobalConfig)
- `~/.coco/settings.json` (user settings)
- `.claude/settings.json` (project settings)
- `.claude/settings.local.json` (local settings)
- Managed/enterprise settings
- `~/.coco/cache/` (model capabilities cache)
- Model selection, provider config, effort, fast mode

coco-config does NOT own: CLAUDE.md (coco-context), .mcp.json (coco-mcp), skills/ (coco-skills), etc.
See `config-file-map.md` for the full ownership map.

## Dependencies

```
coco-config depends on:
  - coco-types           (PermissionMode, ModelRole, ModelSpec, ProviderApi, Capability, etc.)
  - coco-error           (ConfigError)
  - utils/common         (find_coco_home(), LoggingConfig — home dir discovery, REUSE)
  - utils/file-watch     (FileWatcher, FileWatcherBuilder — config file watching, REUSE)
  - utils/absolute-path  (AbsolutePathBuf, ~ expansion — path normalization, REUSE)
  - serde, serde_json    (deserialization)

coco-config does NOT depend on:
  - notify directly (uses utils/file-watch wrapper instead)
  - dirs directly (uses utils/common home dir discovery instead)
  - coco-inference, coco-tool, or any app/ crate
```

## Modules

```
coco-config/src/
  lib.rs
  global_config.rs      # GlobalConfig (~/.coco.json) — user ID, theme, auth, projects
  settings/
    mod.rs              # Settings struct, SettingsWithSource
    schema.rs           # Zod-equivalent serde validation
    loader.rs           # Layered loading: plugin < user < project < local < flag < policy
    watcher.rs          # File watcher via utils/file-watch (debounce, coalesce)
    source.rs           # SettingSource enum, source tracking
    merge.rs            # Deep merge with array dedup
    managed.rs          # Enterprise/MDM policy loading
    validation.rs       # Validation errors, unknown field preservation
    migration.rs        # Settings format migrations
  provider/
    mod.rs              # ProviderConfig, per-provider env_key resolution
    builtin.rs          # Built-in provider definitions (Anthropic, OpenAI, Google, etc.)
  model/
    mod.rs              # get_main_loop_model(), model selection
    configs.rs          # ALL_MODEL_CONFIGS (per-provider model IDs)
    aliases.rs          # ModelAlias resolution
    capabilities.rs     # ModelInfo, Capability checks
    agent.rs            # Subagent model selection
  effort.rs             # ThinkingLevel support checks (TS effort.ts)
  fast_mode.rs          # FastModeState, cooldown
  env.rs                # Environment variable helpers (is_env_truthy, etc.)
  constants.rs          # System constants
```

---

## 0. GlobalConfig (~/.coco.json) — TS loads this separately

TS loads `~/.claude.json` (via `getGlobalConfig()` in `utils/config.ts`). This is **NOT** settings.json — it's a separate file for user-level state.

```rust
/// Per-user global config. Separate from Settings.
/// TS: GlobalConfig type in utils/config.ts, stored at ~/.claude.json
/// Rust: stored at ~/.coco.json (or COCO_CONFIG_DIR/.coco.json)
///
/// Uses utils/common::find_coco_home() for home dir discovery.
#[derive(Serialize, Deserialize, Default)]
pub struct GlobalConfig {
    pub user_id: Option<String>,
    pub theme: Option<String>,
    pub projects: HashMap<String, ProjectConfig>,  // keyed by project path hash
    pub session_costs: HashMap<String, SessionCostState>,
    pub companion: Option<CompanionConfig>,         // buddy pet state
    // ... auth tokens, onboarding state, etc.
}

/// Per-project config within GlobalConfig.
pub struct ProjectConfig {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub costs: Option<SessionCostState>,
}

/// Load with file locking (prevents corruption from concurrent processes).
/// TS: uses lockfile.ts for atomic reads/writes.
pub fn load_global_config() -> Result<GlobalConfig, ConfigError>;
pub fn write_global_config(config: &GlobalConfig) -> Result<(), ConfigError>;
```

### File paths (using utils/common + utils/absolute-path)

```rust
/// Uses utils/common::find_coco_home() — respects COCO_CONFIG_DIR env
pub fn global_config_path() -> AbsolutePathBuf {
    find_coco_home().join(".coco.json")  // ~/.coco.json
}
pub fn config_home() -> AbsolutePathBuf {
    find_coco_home()  // ~/.coco/ (or COCO_CONFIG_DIR)
}
```

---

## 1. Settings Loading Architecture (TS-first)

### TS design: Layered merge with source tracking

TS loads settings from 6 sources, merging later over earlier. Each source is a JSON file. Env vars are a **separate** override layer (not in the merged settings).

### SettingSource enum

```rust
/// Where a setting came from. Used for conflict resolution and security.
/// TS: SettingSource type in settings.ts
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SettingSource {
    Plugin,     // Plugin-contributed base settings (lowest priority)
    User,       // ~/.coco/settings.json (global per machine)
    Project,    // .claude/settings.json (shared, checked in)
    Local,      // .claude/settings.local.json (gitignored)
    Flag,       // --settings CLI file or SDK inline
    Policy,     // Enterprise/MDM managed (highest priority)
}
```

### Settings struct

```rust
/// The merged settings snapshot. Immutable after loading.
/// TS: SettingsJson type in types.ts (Zod schema)
#[derive(Deserialize, Default, Clone)]
#[serde(default, deny_unknown_fields)]  // strict validation
pub struct Settings {
    // === Auth ===
    pub api_key_helper: Option<String>,
    pub force_login_method: Option<LoginMethod>,

    // === Permissions ===
    pub permissions: PermissionsConfig,

    // === Model ===
    pub model: Option<String>,                              // user-specified model
    pub available_models: Option<Vec<String>>,              // enterprise allowlist
    pub model_overrides: Option<HashMap<String, String>>,   // canonical -> provider-specific
    pub thinking_level: Option<ThinkingLevel>,
    pub fast_mode: Option<bool>,
    pub always_thinking_enabled: Option<bool>,

    // === Environment ===
    pub env: HashMap<String, String>,   // injected into shell execution

    // === Hooks ===
    pub hooks: Option<Value>,  // deserialized by coco-hooks, not typed here (avoids L1->L4 dep)
    pub disable_all_hooks: bool,

    // === MCP ===
    pub allowed_mcp_servers: Vec<AllowedMcpServerEntry>,
    pub denied_mcp_servers: Vec<DeniedMcpServerEntry>,
    pub enable_all_project_mcp_servers: bool,

    // === Shell ===
    pub default_shell: Option<ShellKind>,   // Bash (default), PowerShell — defined in coco-context

    // === Display ===
    pub output_style: Option<String>,
    pub language: Option<String>,
    pub syntax_highlighting_disabled: bool,

    // === Plugins ===
    pub enabled_plugins: HashMap<String, PluginConfig>,

    // === Worktree ===
    pub worktree: Option<WorktreeConfig>,

    // === Plans ===
    pub plans_directory: Option<String>,

    // === Auto-Mode ===
    pub auto_mode: Option<AutoModeConfig>,

    // === Attribution ===
    pub include_co_authored_by: Option<bool>,
    pub include_git_instructions: Option<bool>,
}

/// Auto-mode/yolo classifier user configuration.
/// Read by coco-permissions for classifier prompt construction.
#[derive(Deserialize, Default, Clone)]
pub struct AutoModeConfig {
    pub allow: Vec<String>,       // Safe patterns (e.g., "git commits")
    pub soft_deny: Vec<String>,   // Suspicious patterns (e.g., "file deletion")
    pub environment: Vec<String>, // Context (e.g., "building web app")
}

#[derive(Deserialize, Default, Clone)]
pub struct PermissionsConfig {
    pub allow: Vec<PermissionRule>,
    pub deny: Vec<PermissionRule>,
    pub ask: Vec<PermissionRule>,
    pub default_mode: Option<PermissionMode>,
    pub disable_bypass_mode: bool,
    pub additional_directories: Vec<String>,
}
```

### SettingsWithSource (tracks which source provided each field)

```rust
/// Settings snapshot with per-field source tracking.
/// Used for security (project settings can't set certain fields).
pub struct SettingsWithSource {
    pub merged: Settings,
    pub per_source: HashMap<SettingSource, Settings>,  // raw per-source before merge
}

impl SettingsWithSource {
    /// Check if a field was set by a specific source.
    pub fn source_of(&self, field: &str) -> Option<SettingSource> { ... }
}
```

---

## 2. Settings Loading Chain

```rust
/// TS: loadSettingsFromDisk() in settings.ts
/// Merge order (later overrides earlier):
///   1. Plugin base (contributed by enabled plugins)
///   2. User global (~/.coco/settings.json)
///   3. Project shared (.claude/settings.json)
///   4. Project local (.claude/settings.local.json, gitignored)
///   5. Flag (--settings file or SDK inline)
///   6. Policy (enterprise managed, highest priority)
///
/// Merge algorithm: deep merge, arrays concatenated + deduped.
pub fn load_settings(cwd: &Path, flag_settings: Option<&Path>) -> Result<SettingsWithSource, ConfigError>;
```

### File paths (reuses utils/common + utils/absolute-path)

```rust
use coco_utils_common::find_coco_home;    // REUSE: home dir discovery
use coco_utils_absolute_path::AbsolutePathBuf;  // REUSE: path normalization

/// TS: getClaudeConfigHomeDir()
/// Uses utils/common::find_coco_home() — respects COCO_CONFIG_DIR env.
/// DO NOT reimplement home dir logic — utils/common already handles this.
pub fn config_home() -> AbsolutePathBuf {
    find_coco_home()  // ~/.coco or $COCO_CONFIG_DIR
}

pub fn user_settings_path() -> AbsolutePathBuf { config_home().join("settings.json") }
pub fn project_settings_path(cwd: &Path) -> AbsolutePathBuf {
    AbsolutePathBuf::new(cwd.join(".claude/settings.json"))
}
pub fn local_settings_path(cwd: &Path) -> AbsolutePathBuf {
    AbsolutePathBuf::new(cwd.join(".claude/settings.local.json"))
}

/// Enterprise/MDM managed settings (platform-specific)
/// TS: managedPath.ts
pub fn managed_settings_path() -> AbsolutePathBuf {
    #[cfg(target_os = "macos")]
    { AbsolutePathBuf::new("/Library/Application Support/CoCo/managed-settings.json") }
    #[cfg(target_os = "linux")]
    { AbsolutePathBuf::new("/etc/coco/managed-settings.json") }
    #[cfg(target_os = "windows")]
    { AbsolutePathBuf::new(r"C:\Program Files\CoCo\managed-settings.json") }
}
/// Plus managed-settings.d/*.json (alphabetically sorted drop-in fragments)
```

### Policy loading (enterprise)

```rust
/// TS: "first source wins" — highest-priority source provides ALL policy settings.
/// Sources in order: remote > MDM/plist/HKLM > file > HKCU
pub fn load_policy_settings() -> Option<Settings> {
    // 1. Try remote managed (cached from API sync)
    // 2. Try OS-level MDM (macOS plist / Windows HKLM)
    // 3. Try file-based managed-settings.json + .d/
    // Return first non-empty source
}
```

---

## 3. Environment Variable Design

### Pattern: env vars are a separate override layer, NOT inside Settings struct

TS does not merge env vars into the Settings struct. Instead, each subsystem checks both config and env at resolution time. Rust follows the same pattern.

### Env-only variables (no config file equivalent)

```rust
/// Env-only config. No Settings file equivalent.
/// TS is Anthropic-centric; Rust extends for multi-provider.
pub struct EnvOnlyConfig {
    // === Anthropic deployment routing (from TS) ===
    // These select WHICH Anthropic endpoint, not which provider.
    pub use_bedrock: bool,           // CLAUDE_CODE_USE_BEDROCK (AWS)
    pub use_vertex: bool,            // CLAUDE_CODE_USE_VERTEX (GCP)
    pub use_foundry: bool,           // CLAUDE_CODE_USE_FOUNDRY (Azure)

    // === Model override (higher priority than settings.model) ===
    pub model_override: Option<String>,  // ANTHROPIC_MODEL (or COCO_MODEL for multi-provider)
    pub small_fast_model: Option<String>,// ANTHROPIC_SMALL_FAST_MODEL
    pub subagent_model: Option<String>,  // CLAUDE_CODE_SUBAGENT_MODEL

    // === Shell ===
    pub shell_override: Option<String>,  // CLAUDE_CODE_SHELL

    // === Limits ===
    pub max_tool_concurrency: Option<i32>,  // CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY
    pub max_context_tokens: Option<i64>,    // CLAUDE_CODE_MAX_CONTEXT_TOKENS

    // === Runtime flags ===
    pub bare_mode: bool,              // CLAUDE_CODE_SIMPLE=1 or --bare
}

impl EnvOnlyConfig {
    /// Read all env vars once at startup. Uses utils/env helpers.
    pub fn from_env() -> Self {
        Self {
            use_bedrock: is_env_truthy("CLAUDE_CODE_USE_BEDROCK"),
            use_vertex: is_env_truthy("CLAUDE_CODE_USE_VERTEX"),
            use_foundry: is_env_truthy("CLAUDE_CODE_USE_FOUNDRY"),
            model_override: std::env::var("ANTHROPIC_MODEL").ok(),
            // ...
        }
    }
}
```

**Note**: `EnvOnlyConfig` handles Anthropic routing (from TS). For multi-provider API keys, see `ProviderConfig.env_key` below — each provider resolves its own env var.

### Per-provider configuration (multi-provider)

Authoritative on the three-layer boundary, on-disk wire format, and resolution: see [`multi-provider-plan.md`](multi-provider-plan.md). What lives here is the **type catalogue**.

#### `RedactedSecret` — secret newtype (Layer 1 invariant)

```rust
/// Secret string that never round-trips through `Debug` / `Display` / `format!`.
/// Use `.expose()` at the single call-site that builds the auth header.
///
/// Defence-in-depth against `tracing::error!("{cfg:?}")`, snafu cause chains,
/// `.expect("config: {cfg:?}")`, assertion-failure formatters, and panic
/// backtraces — all of which go through `Debug`/`Display` BEFORE reaching the
/// log-sink-level `secret-redact` post-processor.
#[derive(Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RedactedSecret(String);

impl RedactedSecret {
    pub fn expose(&self) -> &str { &self.0 }
}
impl fmt::Debug for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedSecret(<redacted>)")
    }
}
impl fmt::Display for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
```

`grep -r '\.expose()' coco-rs/` enumerates every site where a secret leaves the type — should be 1–2 sites in `app/cli/src/model_factory.rs`.

#### `PartialProviderConfig` (wire) and `ProviderConfig` (resolved)

```rust
/// Wire format — every field optional so omission means "inherit".
/// `BTreeMap` (NOT `HashMap`) so serialised output, snapshots, and review diffs are stable.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialProviderConfig {
    // NOTE: identity is the parent map key. Intentionally NO `name` field —
    // `serde(deny_unknown_fields)` rejects user-written `name` at parse time.
    pub api:            Option<ProviderApi>,
    pub env_key:        Option<String>,
    pub api_key:        Option<RedactedSecret>,
    pub base_url:       Option<String>,
    pub default_model:  Option<String>,
    pub timeout_secs:   Option<i64>,
    pub streaming:      Option<bool>,
    pub wire_api:       Option<WireApi>,
    pub client_options: Option<PartialProviderClientOptions>,
    pub models:         Option<BTreeMap<String, PartialProviderModelOverride>>,
}
impl fmt::Debug for PartialProviderConfig { /* prints api_key as "<redacted>" */ }

/// Resolved form — required fields concrete; only genuinely-optional fields stay Option.
#[derive(Clone)]   // NOTE: no `Debug` derive — see custom impl
pub struct ProviderConfig {
    pub name:           String,                 // = parent map key (set in from_partial)
    pub api:            ProviderApi,
    pub env_key:        String,
    pub api_key:        Option<RedactedSecret>,
    pub base_url:       String,
    pub default_model:  Option<String>,
    pub timeout_secs:   i64,
    pub streaming:      bool,
    pub wire_api:       WireApi,
    pub client_options: ProviderClientOptions,
    pub models:         BTreeMap<String, ProviderModelOverride>,
}
impl fmt::Debug for ProviderConfig { /* redacts api_key */ }

impl ProviderConfig {
    /// Construct a resolved config from a partial overlay against a fresh slate.
    /// `name` is taken from `map_key`, never from the overlay.
    pub fn from_partial(
        map_key: &str,
        partial: &PartialProviderConfig,
    ) -> Result<Self, ConfigError>;

    /// Resolve API key: env var (via `env_key`) → `api_key` → typed error.
    pub fn resolve_api_key(&self) -> Result<RedactedSecret, ConfigError>;

    /// Apply a partial overlay to an existing resolved config.
    /// Each `Some(_)` field on the partial wins; `None` keeps the base value.
    /// **Never coerces `api` to a serde default.**
    pub fn merge_partial(&mut self, partial: &PartialProviderConfig);
}

/// Built-in providers (compiled-in registry, lazy `OnceLock`).
pub fn builtin_providers() -> &'static BTreeMap<String, ProviderConfig>;
```

#### `PartialProviderClientOptions` and `ProviderClientOptions`

```rust
/// Wire format — `BTreeMap`-ordered, every field `Option`. Typed parser rejects
/// unknown keys at parse time with JSON-pointer error messages.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub struct PartialProviderClientOptions {
    pub headers:                       Option<BTreeMap<String, String>>,
    pub auth_token:                    Option<RedactedSecret>,
    pub organization_id:               Option<String>,
    pub project_id:                    Option<String>,
    pub include_usage:                 Option<bool>,
    pub full_url:                      Option<bool>,
    pub supports_structured_outputs:   Option<bool>,
}

#[derive(Clone, Default)]
pub struct ProviderClientOptions {
    pub headers:                       BTreeMap<String, String>,
    pub auth_token:                    Option<RedactedSecret>,
    pub organization_id:               Option<String>,
    pub project_id:                    Option<String>,
    pub include_usage:                 Option<bool>,        // None = SDK default (false)
    pub full_url:                      bool,
    pub supports_structured_outputs:   bool,
}
impl fmt::Debug for ProviderClientOptions { /* redacts auth_token */ }
```

True provider pass-through (HTTP-body fields not modelled here) goes through `ModelInfo.extra_body`. Headers go through `client_options.headers`. There is intentionally no generic "anything goes" map at the provider level.

### Dual-source settings (both config file AND env var)

```rust
/// For settings that exist in BOTH config file and env var,
/// resolution order is explicitly documented per-setting.
///
/// Model: /model cmd > --model flag > ANTHROPIC_MODEL env > settings.model > default
/// API key: ANTHROPIC_API_KEY env > apiKeyHelper script > OAuth token
/// Effort: CLAUDE_CODE_EFFORT_LEVEL env > settings.effort_level > model default
```

### Env helper utilities (from `envUtils.ts`)

```rust
/// TS: isEnvTruthy() — normalizes "1", "true", "yes", "on" to true
pub fn is_env_truthy(key: &str) -> bool;

/// TS: isEnvDefinedFalsy() — normalizes "0", "false", "no", "off" to false
pub fn is_env_falsy(key: &str) -> bool;
```

---

## 4. Runtime Overrides (session state, not persisted)

```rust
/// Mutable state that changes during a session.
/// NOT persisted to config files. Lost when session ends.
/// TS: bootstrap/state.ts mainLoopModelOverride, etc.
pub struct RuntimeOverrides {
    pub model_override: Option<String>,         // /model command
    pub thinking_level_override: Option<ThinkingLevel>,  // /effort or /think command
    pub fast_mode_override: Option<bool>,       // /fast command
    pub permission_mode_override: Option<PermissionMode>,  // plan mode toggle
}

impl RuntimeOverrides {
    pub fn new() -> Self { Self::default() }
}
```

---

## 5. Config Resolution (combining all layers)

```rust
/// The fully resolved configuration for a session.
/// Combines: persisted Settings + EnvOnlyConfig + RuntimeOverrides.
pub struct ResolvedConfig {
    pub settings: Settings,                // merged from disk (immutable snapshot)
    pub env: EnvOnlyConfig,                // from environment
    pub overrides: RuntimeOverrides,       // mutable session state
    pub source_tracking: SettingsWithSource,
}

impl ResolvedConfig {
    /// Get the effective model for a role.
    /// Priority: override > env > settings > default
    pub fn model_for_role(&self, role: ModelRole) -> ModelSpec { ... }

    /// Get the effective API provider.
    /// TS: getAPIProvider()
    pub fn api_provider(&self) -> ProviderApi {
        if self.env.use_bedrock { return ProviderApi::Anthropic; } // Bedrock variant
        if self.env.use_vertex { return ProviderApi::Anthropic; }  // Vertex variant
        if self.env.use_foundry { return ProviderApi::Anthropic; } // Foundry variant
        // ... or from config for OpenAI/Google/etc.
        ProviderApi::Anthropic // default
    }

    /// Get the effective effort level.
    /// Priority: override > env > settings > model default
    pub fn effective_thinking_level(&self, model: &str) -> Option<ThinkingLevel> { ... }
}
```

---

## 6. Model Configuration

### Bounded numeric newtypes (Layer 1 invariant)

```rust
/// Token-count metadata that must be a positive int. Internal repr `u32` so
/// downstream `From<PositiveTokens> for u64` is infallible — eliminates the
/// `as u64` underflow footgun across the entire call chain.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PositiveTokens(u32);

impl TryFrom<i64> for PositiveTokens {
    type Error = ConfigError;
    fn try_from(v: i64) -> Result<Self, ConfigError> {
        u32::try_from(v).map(Self).map_err(|_| ConfigError::NonPositiveTokens { value: v })
    }
}
impl<'de> Deserialize<'de> for PositiveTokens { /* via i64 + TryFrom */ }
impl From<PositiveTokens> for u64 { fn from(v: PositiveTokens) -> u64 { v.0 as u64 } }
impl From<PositiveTokens> for i64 { fn from(v: PositiveTokens) -> i64 { v.0 as i64 } }

/// Same shape, used for `top_k` / similar small positive ints.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PositiveCount(u32);
impl TryFrom<i64> for PositiveCount { /* same pattern */ }
```

JSON callers naturally write `200000` (parses as `i64`); we keep the wire format `i64` and validate at the type boundary.

### `PartialModelInfo` (wire) and `ModelInfo` (resolved)

```rust
/// Wire format — Option distinguishes "unset" from "explicitly set".
/// `BTreeMap` so serialised output is deterministic for snapshots.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialModelInfo {
    // NOTE: model_id is the parent map key (same identity invariant as ProviderConfig).
    pub display_name:              Option<String>,
    pub context_window:            Option<PositiveTokens>,
    pub max_output_tokens:         Option<PositiveTokens>,
    pub timeout_secs:              Option<i64>,
    pub capabilities:              Option<Vec<Capability>>,
    pub temperature:               Option<f32>,
    pub top_p:                     Option<f32>,
    pub top_k:                     Option<PositiveCount>,
    pub supported_thinking_levels: Option<Vec<ThinkingLevel>>,
    pub default_thinking_level:    Option<ReasoningEffort>,
    pub auto_compact_pct:          Option<i32>,
    pub apply_patch_tool_type:     Option<ApplyPatchToolType>,
    pub tool_overrides:            Option<ToolOverrides>,
    pub shell_type:                Option<ConfigShellToolType>,
    pub max_tool_output_chars:     Option<i32>,
    pub base_instructions:         Option<String>,
    pub base_instructions_file:    Option<String>,
    pub extra_body:                Option<BTreeMap<String, JSONValue>>,
}

/// Resolved form — context_window / max_output_tokens are required-and-positive.
/// temperature / top_p / top_k stay Option because None == "let the provider default";
/// see multi-provider-plan.md §7.1.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub model_id:                  String,                 // = parent map key
    pub display_name:              Option<String>,
    pub context_window:            PositiveTokens,
    pub max_output_tokens:         PositiveTokens,
    pub timeout_secs:              Option<i64>,
    pub capabilities:              Option<Vec<Capability>>,
    pub temperature:               Option<f32>,
    pub top_p:                     Option<f32>,
    pub top_k:                     Option<PositiveCount>,
    pub supported_thinking_levels: Option<Vec<ThinkingLevel>>,
    pub default_thinking_level:    Option<ReasoningEffort>,
    pub auto_compact_pct:          Option<i32>,
    pub apply_patch_tool_type:     Option<ApplyPatchToolType>,
    pub tool_overrides:            Option<ToolOverrides>,
    pub shell_type:                Option<ConfigShellToolType>,
    pub max_tool_output_chars:     Option<i32>,
    pub base_instructions:         Option<String>,
    pub base_instructions_file:    Option<String>,
    /// Layer 1 escape hatch. Provider-agnostic flat keys, **camelCase to match
    /// each provider's typed-options struct rename_all attribute**
    /// (multi-provider-plan.md §7.4). Layer 2 wraps as
    /// `provider_options[<provider_name>]` at call time.
    pub extra_body:                BTreeMap<String, JSONValue>,
}

impl ModelInfo {
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.as_ref().is_some_and(|caps| caps.contains(&cap))
    }

    pub fn default_thinking(&self) -> Option<&ThinkingLevel> {
        let effort = self.default_thinking_level?;
        self.supported_thinking_levels.as_ref()?
            .iter()
            .find(|l| l.effort == effort)
    }

    pub fn resolve_thinking_level(&self, requested: &ThinkingLevel) -> ThinkingLevel {
        match &self.supported_thinking_levels {
            Some(levels) if !levels.is_empty() => {
                levels.iter()
                    .find(|l| l.effort == requested.effort)
                    .cloned()
                    .unwrap_or_else(|| {
                        levels.iter()
                            .min_by_key(|l| (l.effort as i32 - requested.effort as i32).abs())
                            .cloned()
                            .unwrap_or_else(|| requested.clone())
                    })
            }
            _ => requested.clone(),
        }
    }

    /// Validate Partial → resolved. Required-and-positive fields fail loudly.
    pub fn from_partial(
        provider: &str,
        model_id: &str,
        p: PartialModelInfo,
    ) -> Result<Self, ConfigError>;

    /// Merge another resolved config into this one (other.Some overrides self).
    pub fn merge_from(&mut self, other: &Self);
}
```

### `PartialProviderModelOverride` and `ProviderModelOverride`

```rust
/// Per-(provider, model) override layer — overrides any builtin/catalog ModelInfo
/// for the (provider, model) pair. Rejected alternative `ProviderModelEntry`
/// was ambiguous ("entry of what?"). New (provider, model) pairs that lack a
/// builtin/catalog still go through the same struct — an override against an
/// empty base is well-defined.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialProviderModelOverride {
    /// API model name if different from `model_id` (e.g., Bedrock endpoint ID).
    pub api_model_name:  Option<String>,
    /// Per-(provider, model) ModelInfo deltas — apply on top of the catalog ModelInfo.
    #[serde(flatten)]
    pub model_info_overlay: PartialModelInfo,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderModelOverride {
    pub api_model_name:  Option<String>,
    pub model_info_overlay: PartialModelInfo,   // applied at registry build time
}

impl PartialProviderModelOverride {
    /// Apply this entry's deltas onto an in-progress PartialModelInfo accumulator.
    pub fn apply_to(&self, acc: &mut PartialModelInfo);
}
```

### `ResolvedModel`, `ModelRegistry` (closes the L1 dormant gap)

```rust
/// One model fully resolved against a single (provider, model_id) pair.
/// Built once at config-load time; consulted O(1) by RuntimeConfig clients.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub info:           ModelInfo,
    pub provider_model: ProviderModelOverride,
}

#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    resolved: BTreeMap<(String, String), Arc<ResolvedModel>>,  // (provider, model_id)
}

impl ModelRegistry {
    pub fn resolve(&self, provider: &str, model_id: &str) -> Option<&Arc<ResolvedModel>>;
    pub fn iter(&self) -> impl Iterator<Item = (&(String, String), &Arc<ResolvedModel>)>;
}

/// Build the registry by walking every (provider, model_id) pair and merging
/// builtin → ~/.coco/models.json → provider_cfg.models[<id>].
pub fn build_model_registry(
    providers:    &BTreeMap<String, ProviderConfig>,
    user_catalog: &BTreeMap<String, PartialModelInfo>,
    coco_home:    &Path,
) -> Result<ModelRegistry, ConfigError>;
```

### ModelRoles (role -> model mapping)

```rust
pub struct ModelRoles {
    pub roles: HashMap<ModelRole, ModelSpec>,
}

impl ModelRoles {
    pub fn get(&self, role: ModelRole) -> &ModelSpec {
        self.roles.get(&role).unwrap_or_else(|| &self.roles[&ModelRole::Main])
    }
}
```

### `RuntimeConfig` — atomic snapshot consumed by `QueryEngine` and `model_factory`

```rust
/// One atomic snapshot of all resolved config. Built by `build_runtime_config`,
/// distributed via `tokio::sync::watch::Sender<Arc<RuntimeConfig>>`.
/// Subscribers borrow at turn boundaries; in-flight turns retain the captured
/// `Arc` and never observe a torn read.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub providers:      BTreeMap<String, ProviderConfig>,
    pub model_roles:    ModelRoles,
    pub features:       Features,
    pub model_registry: Arc<ModelRegistry>,
    /// Computed from the Main role's ResolvedModel.tool_overrides at config-build
    /// time (closes the L1 dormant gap from `runtime.rs:141-156`).
    pub tool_overrides: Arc<ToolOverrides>,
    /// Provider-neutral cache config (currently 1h-TTL allowlist). Read by
    /// `vercel-ai-anthropic` at provider construction. Hashed into the
    /// fingerprint's `runtime_state_digest` so settings reload that mutates
    /// the allowlist invalidates the cached `Arc<dyn LanguageModelV4>`.
    pub prompt_cache: PromptCacheRuntimeConfig,
    // Anthropic-specific knobs (experimental_betas, disable_interleaved_thinking,
    // show_thinking_summaries, non_interactive) moved to `ProviderConfig.provider_options`
    // (per-provider opaque map). The adapter parses them via
    // `vercel-ai-anthropic::parse_provider_options`. See R7-10 in audit-gaps.md.
    /// Auth/billing identity. **Session-stable** (R3-F3) — read by
    /// `vercel-ai-anthropic` at provider construction; latched into
    /// `CachePolicy::eligible_1h` on first call.
    pub account: AccountConfig,
}

pub fn build_runtime_config(
    settings:     &Settings,
    file_catalog_providers: &BTreeMap<String, PartialProviderConfig>,
    file_catalog_models:    &BTreeMap<String, PartialModelInfo>,
    coco_home:    &Path,
) -> Result<RuntimeConfig, ConfigError>;
```

### Model Selection Priority (from `utils/model/model.ts`)

```rust
/// Resolution order:
/// 1. RuntimeOverrides.model_override (/model command)
/// 2. CLI --model flag (stored in EnvOnlyConfig at startup)
/// 3. ANTHROPIC_MODEL env var (EnvOnlyConfig.model_override)
/// 4. Settings.model (merged config file field)
/// 5. Default by subscription tier / provider
pub fn get_main_loop_model(config: &ResolvedConfig) -> String;

/// Subagent: CLAUDE_CODE_SUBAGENT_MODEL env > tool model > agent model > "inherit"
pub fn get_agent_model(
    agent_model: Option<&str>,
    parent_spec: &ModelSpec,
    tool_model: Option<&str>,
    config: &ResolvedConfig,
) -> ModelSpec;
```

### Model Aliases

```rust
pub enum ModelAlias {
    Sonnet, Opus, Haiku, Best, SonnetLargeCtx, OpusLargeCtx, OpusPlan,
}

pub fn resolve_alias(alias: ModelAlias, provider: ProviderApi) -> String;
pub fn parse_user_model(input: &str) -> Result<String, ConfigError>;
```

---

## 7. Settings + Catalog Watching (reuses utils/file-watch)

```rust
/// Uses utils/file-watch::FileWatcherBuilder (from cocode-rs, REUSE).
/// TS: changeDetector.ts with 1000ms stability threshold, 500ms poll.
///
/// DO NOT use notify directly — utils/file-watch already provides
/// event coalescing, throttling, and debounce.
use coco_file_watch::{FileWatcher, FileWatcherBuilder};

pub struct SettingsWatcher {
    inner:     FileWatcher,                                     // from utils/file-watch
    publisher: tokio::sync::watch::Sender<Arc<RuntimeConfig>>,  // hot-reload broadcast
}

impl SettingsWatcher {
    /// Watches the 4 settings paths AND the 2 sibling catalog files. Any change
    /// triggers a single debounced rebuild of `RuntimeConfig`; the new `Arc` is
    /// published via the watch channel.
    ///
    /// In-flight turns continue with the pre-reload `Arc`; the next turn picks
    /// up the fresh snapshot atomically. Provider-client coherence at the next
    /// turn is enforced by `coco-inference::ProviderClientFingerprint`
    /// (multi-provider-plan.md §11.1).
    pub fn new(
        cwd: &Path,
        coco_home: &Path,
        publisher: tokio::sync::watch::Sender<Arc<RuntimeConfig>>,
    ) -> Result<Self, ConfigError> {
        let publisher_for_change = publisher.clone();
        let watcher = FileWatcherBuilder::new()
            .debounce_ms(1000)                                  // TS: 1000ms stability threshold
            .watch(user_settings_path())
            .watch(project_settings_path(cwd))
            .watch(local_settings_path(cwd))
            .watch(managed_settings_path())
            .watch(coco_home.join("providers.json"))            // catalog file
            .watch(coco_home.join("models.json"))               // catalog file
            .on_change(move |_path| {
                if let Ok(rc) = build_runtime_config_from_disk() {
                    let _ = publisher_for_change.send(Arc::new(rc));
                }
            })
            .build()?;
        Ok(Self { inner: watcher, publisher })
    }
}
```

---

## 8. Effort & Fast Mode

```rust
pub fn model_supports_effort(model_info: &ModelInfo) -> bool {
    model_info.supports(Capability::Effort)
}
pub fn model_supports_max_effort(model_info: &ModelInfo) -> bool {
    // Opus 4.6 only
    model_info.model_id.contains("opus-4-6")
}
pub fn get_default_thinking_level(model_info: &ModelInfo) -> Option<ThinkingLevel>;

/// Fast mode: same model (Opus 4.6), faster output speed. NOT a model switch.
pub enum FastModeState {
    Active,
    Cooldown { reset_at: i64, reason: CooldownReason },
}
pub enum CooldownReason { RateLimit, Overloaded }

/// Org-level availability check (from utils/fastMode.ts 533 LOC).
/// Returns (available, reason) where reason explains why not available.
/// Unavailability reasons:
///   "free" — free account tier (fast mode requires paid subscription)
///   "preference" — organization disabled fast mode via policy
///   "extra_usage_disabled" — requires billing, not enabled
///   "network_error" — org check failed (behind proxy)
///   "unknown" — unexpected failure
pub fn get_fast_mode_unavailable_reason(config: &ResolvedConfig) -> Option<String>;
pub fn is_fast_mode_available(config: &ResolvedConfig) -> bool;
pub fn get_fast_mode_model() -> String;  // "opus" or "opus[1m]"

/// Cooldown trigger: 429 (rate limit) or 503 (overloaded) during fast mode.
/// Duration: From reset_at timestamp (typically minutes).
/// Reset: Automatic when now >= reset_at.
pub fn trigger_cooldown(reason: CooldownReason, reset_at: i64);

/// Org status check endpoint: {BASE_API_URL}/api/claude_code_penguin_mode
/// Throttle: 30s minimum between requests.
/// Cache: penguinModeOrgEnabled persisted in global config.
/// Prefetch: runs at startup (prefetchFastModeStatus).
///   Checks auth scope (user:profile), OAuth vs API key.
///   30s throttle interval, in-flight promise memoization.
pub async fn prefetch_fast_mode_status();

/// Overage rejection: called when 429 includes
/// anthropic-ratelimit-unified-overage-disabled-reason header.
/// Reasons: out_of_credits, org_level_disabled, member_level_disabled,
///          seat_tier_level_disabled, overage_not_provisioned, etc.
/// Permanently disables fast mode (except out_of_credits/org_level_disabled_until).
pub fn handle_fast_mode_overage_rejection(reason: Option<&str>);

/// Per-session opt-in: when settings.fast_mode_per_session_opt_in is true,
/// fast mode starts OFF each session (user must explicitly enable with /fast).
pub fn get_initial_fast_mode_setting(
    model: &str,
    per_session_opt_in: bool,
) -> bool;

/// 1m merge: getFastModeModel() returns "opus" or "opus[1m]"
/// depending on is_opus_1m_merge_enabled() (context window expansion).
pub fn get_fast_mode_state(
    model: &str,
    user_enabled: Option<bool>,
) -> FastModeDisplayState;  // Off, Cooldown, On

pub enum FastModeDisplayState { Off, Cooldown, On }
```

---

## 9. Security Rules (from TS)

```rust
/// Security constraints on what project settings can do.
/// TS: project settings are restricted for security.
///
/// Project settings (.claude/settings.json) CANNOT set:
/// - apiKeyHelper (prevents RCE via path traversal)
/// - autoMemoryDirectory
/// - bypass permissions mode
/// - auto mode config
///
/// Only Policy settings can enforce:
/// - strictPluginOnlyCustomization (locks skills/hooks/agents to plugin-only)
/// - allowManagedPermissionRulesOnly
/// - allowManagedMcpServersOnly
```
