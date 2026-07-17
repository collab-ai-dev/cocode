use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use coco_types::Features;
use coco_types::ModelRole;
use coco_types::ModelSpec;
use coco_types::ProviderModelSelection;
use coco_types::ToolOverrides;

use crate::builtin::builtin_providers;
use crate::compact_settings::CompactConfig;
use crate::env::EnvOnlyConfig;
use crate::env::EnvSnapshot;
use crate::error::ConfigError;
use crate::event_hub::EventHubConfig;
use crate::model::MAX_REFERENCE_MODELS;
use crate::model::MOA_PROVIDER;
use crate::model::MoaEndpointSpec;
use crate::model::MoaPresetSettings;
use crate::model::MoaSettings;
use crate::model::ModelRegistry;
use crate::model::ModelRoles;
use crate::model::PartialModelInfo;
use crate::model::RoleSlot;
use crate::model::RoleSlots;
use crate::model::build_model_registry;
use crate::model::moa;
use crate::model_allowlist::is_model_allowed;
use crate::overrides::RuntimeOverrides;
use crate::prompt_cache_settings::AccountConfig;
use crate::prompt_cache_settings::PromptCacheRuntimeConfig;
use crate::provider::PartialProviderConfig;
use crate::provider::ProviderConfig;
use crate::sandbox_settings::SandboxSettings;
use crate::sections::AgentTeamsConfig;
use crate::sections::ApiConfig;
use crate::sections::DiagnosticsConfig;
use crate::sections::LoopConfig;
use crate::sections::LspConfig;
use crate::sections::McpRuntimeConfig;
use crate::sections::MemoryActivation;
use crate::sections::MemoryConfig;
use crate::sections::MemoryDisabledReason;
use crate::sections::OutputRewriteConfig;
use crate::sections::PathConfig;
use crate::sections::ServerConfig;
use crate::sections::ShellConfig;
use crate::sections::SkillLearnConfig;
use crate::sections::ToolConfig;
use crate::sections::VoiceConfig;
use crate::sections::WebFetchConfig;
use crate::sections::WebSearchConfig;
use crate::settings::SettingsRoots;
use crate::settings::SettingsWithSource;
use crate::skill_overrides::SkillOverrideTiers;

/// JSON-first runtime configuration snapshot.
///
/// This is the boundary object leaf crates should consume instead of reading
/// process env or reconstructing defaults themselves. The raw env snapshot
/// is intentionally not retained — every env-derived knob has already been
/// folded into either `env_only` or a typed section, so consumers never
/// need to reach back at the unfiltered snapshot.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub settings: SettingsWithSource,
    pub env_only: EnvOnlyConfig,
    pub overrides: RuntimeOverrides,
    /// Resolved provider catalog. `BTreeMap` so on-disk serialisation
    /// (settings.json overlay round-trip, debug snapshots) is byte-stable.
    pub providers: BTreeMap<String, ProviderConfig>,
    pub model_roles: ModelRoles,
    /// Resolved (provider, model_id) → `Arc<ResolvedModel>` index.
    /// Source of `info.tool_overrides` for the tool-filter pipeline.
    pub model_registry: Arc<ModelRegistry>,
    pub api: ApiConfig,
    pub loop_config: LoopConfig,
    pub tool: ToolConfig,
    pub shell: ShellConfig,
    /// Bash output-compression config (`Feature::OutputRewrite`). Consumed at
    /// session bootstrap to build the session-scoped output rewriter.
    pub output_rewrite: OutputRewriteConfig,
    pub sandbox: SandboxSettings,
    pub memory: MemoryConfig,
    pub memory_activation: MemoryActivation,
    pub mcp: McpRuntimeConfig,
    pub web_fetch: WebFetchConfig,
    pub web_search: WebSearchConfig,
    /// Diagnostics knobs (LLM wire-traffic dumper). Consumed by
    /// `app/query` to build the per-session wire recorder.
    pub diagnostics: DiagnosticsConfig,
    /// LSP tool-layer knobs. Server roster (`lsp_servers.json`) lives
    /// in `coco-lsp`; this struct only carries cross-server tool-side
    /// limits (file-size gate, future timeout / prewarm policy).
    pub lsp: LspConfig,
    /// Voice-input (speech-to-text) knobs. On/off is `Feature::Voice`; this
    /// carries backend/language/model selection. Consumed by `coco-voice`.
    pub voice: VoiceConfig,
    /// Event Hub connector egress settings. `url == None` disables egress.
    pub event_hub: EventHubConfig,
    /// AppServer listener settings used by SDK/local-server entrypoints.
    pub server: ServerConfig,
    pub paths: PathConfig,
    /// Resolved compaction parameters (auto threshold, micro keep-recent,
    /// api-native gate, session-memory budgets, experimental flags). Single
    /// source of truth — `coco_compact` reads this and never touches env.
    pub compact: CompactConfig,
    pub agent_teams: AgentTeamsConfig,
    /// Autonomous skill-learning loop knobs. Single source of truth —
    /// `coco-skill-learn` reads this and never touches env directly.
    pub skill_learn: SkillLearnConfig,
    /// Provider-agnostic prompt-cache settings (1h-TTL allowlist).
    /// Adapter (`vercel-ai-anthropic`) reads `allowlist` via
    /// `AnthropicConfig.prompt_cache_allowlist` (set by `build_anthropic`).
    /// See `docs/internal/prompt-cache-design.md` §16a.
    pub prompt_cache: PromptCacheRuntimeConfig,
    /// Account / billing identity (api_key vs subscriber, in-overage
    /// flag). Drives 1h-TTL eligibility latch + OAuth beta in the
    /// Anthropic adapter. **Session-stable** (R3-F3).
    pub account: AccountConfig,
    /// Coarse-grained capability gates. See
    /// `docs/internal/feature-gates-and-tool-filtering.md`.
    pub features: Features,
    /// Per-tier `skill_overrides` map preserved without merging.
    /// Drives the 4-state Skill tool gate, listing filters, and the
    /// `/skills` dialog. Resolution semantics are non-trivial — see
    /// [`SkillOverrideTiers`] docs and `coco-skills::overrides` for the
    /// three resolvers (`oT5` / `aT5` / `st` mirrors).
    pub skill_overrides: SkillOverrideTiers,
    /// Layer 2 of the tool-filter pipeline — extra tools the active
    /// main-loop model adds beyond the baseline + baseline tools it
    /// excludes. Resolved once from the Main role's `(provider,
    /// model_id)` pair via `model_registry`; subagents inherit this
    /// `Arc` and never widen it.
    pub tool_overrides: Arc<ToolOverrides>,
    /// Which setting sources participate in loading + customization. Resolved
    /// from the `--setting-sources` CSV flag (`None` ⇒ all five). `Policy` and
    /// `Flag` are always present (read-only, admin/CLI controlled). Consumed by
    /// the skill/agent/hook/mcp loaders to skip user/project/local scopes that
    /// the operator disabled.
    pub enabled_setting_sources: std::collections::HashSet<crate::settings::SettingSource>,
}

/// Resolved on-disk paths for settings + catalog files. Threaded
/// through `RuntimeConfigBuilder` so tests can isolate filesystem
/// reads via `tempfile::TempDir`. Production `Default` resolves to
/// the user's `config home/` via `global_config`.
///
/// Every path the resolver and reloader read is overridable here —
/// including user-level `settings.json` and the platform-managed
/// policy file, not just the `providers.json` / `models.json`
/// catalogs.
#[derive(Debug, Clone)]
pub struct CatalogPaths {
    pub coco_home: PathBuf,
    pub user_settings: PathBuf,
    pub managed_settings: PathBuf,
    pub providers: PathBuf,
    pub models: PathBuf,
}

impl Default for CatalogPaths {
    fn default() -> Self {
        Self {
            coco_home: crate::global_config::config_home(),
            user_settings: crate::global_config::user_settings_path(),
            managed_settings: crate::global_config::managed_settings_path(),
            providers: crate::global_config::providers_catalog_path(),
            models: crate::global_config::models_catalog_path(),
        }
    }
}

impl CatalogPaths {
    /// Construct a CatalogPaths rooted at `home` — convenience for tests
    /// that want every path under a single TempDir.
    pub fn rooted(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            user_settings: home.join("settings.json"),
            managed_settings: home.join("managed-settings.json"),
            providers: home.join("providers.json"),
            models: home.join("models.json"),
            coco_home: home,
        }
    }

    /// CatalogPaths whose all files point inside `home` and don't
    /// exist by default. Useful for tests that assert empty-catalog
    /// behavior.
    pub fn empty_in(home: impl Into<PathBuf>) -> Self {
        Self::rooted(home)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfigBuilder {
    settings_roots: crate::settings::SettingsRoots,
    flag_settings: Option<PathBuf>,
    env: EnvSnapshot,
    overrides: RuntimeOverrides,
    catalogs: CatalogPaths,
    /// Raw `--setting-sources` CSV. `None` ⇒ all five sources enabled.
    setting_sources: Option<String>,
}

impl RuntimeConfigBuilder {
    pub fn from_process(cwd: impl Into<PathBuf>) -> Self {
        Self {
            settings_roots: SettingsRoots::from_cwd(cwd),
            flag_settings: None,
            env: EnvSnapshot::from_current_process(),
            overrides: RuntimeOverrides::default(),
            catalogs: CatalogPaths::default(),
            setting_sources: None,
        }
    }

    pub fn new(cwd: impl Into<PathBuf>, env: EnvSnapshot) -> Self {
        Self {
            settings_roots: SettingsRoots::from_cwd(cwd),
            flag_settings: None,
            env,
            overrides: RuntimeOverrides::default(),
            catalogs: CatalogPaths::default(),
            setting_sources: None,
        }
    }

    pub fn with_flag_settings(mut self, path: impl Into<PathBuf>) -> Self {
        self.flag_settings = Some(path.into());
        self
    }

    pub fn with_overrides(mut self, overrides: RuntimeOverrides) -> Self {
        self.overrides = overrides;
        self
    }

    /// Override the catalog paths. Tests pass a `TempDir`-rooted
    /// `CatalogPaths` to isolate from the developer's `config home/`.
    pub fn with_catalog_paths(mut self, catalogs: CatalogPaths) -> Self {
        self.catalogs = catalogs;
        self
    }

    /// Resolve shared project settings from `project_root` and local
    /// settings from `local_root`. Single-cwd callers keep the default.
    pub fn with_settings_roots(
        mut self,
        project_root: impl Into<PathBuf>,
        local_root: impl Into<PathBuf>,
    ) -> Self {
        self.settings_roots = SettingsRoots::new(project_root, local_root);
        self
    }

    /// Restrict which setting sources participate via the `--setting-sources`
    /// CSV (`user`/`project`/`local`/`flag`/`policy`). `None` (the default) ⇒
    /// all five. `Policy` + `Flag` are always force-added downstream.
    pub fn with_setting_sources(mut self, csv: Option<String>) -> Self {
        self.setting_sources = csv;
        self
    }

    pub fn build(self) -> crate::Result<RuntimeConfig> {
        let enabled = parse_enabled_setting_sources(self.setting_sources.as_deref());
        let settings = crate::settings::load_settings_with_roots(
            &self.settings_roots,
            self.flag_settings.as_deref(),
            &self.catalogs.user_settings,
            &self.catalogs.managed_settings,
            &enabled,
        )?;
        build_runtime_config_with(settings, self.env, self.overrides, self.catalogs, enabled)
    }
}

/// Parse the `--setting-sources` CSV into the enabled-source set.
///
/// `None` ⇒ all five sources. An explicit (possibly empty) string parses
/// `user`/`project`/`local`/`flag`/`policy` tokens; unknown tokens are
/// ignored. `Policy` and `Flag` are ALWAYS present — they're admin-managed
/// (read-only) and CLI-supplied, so the operator can never disable them.
pub fn parse_enabled_setting_sources(
    csv: Option<&str>,
) -> std::collections::HashSet<crate::settings::SettingSource> {
    use crate::settings::SettingSource;
    let mut set = std::collections::HashSet::new();
    match csv {
        None => {
            set.insert(SettingSource::User);
            set.insert(SettingSource::Project);
            set.insert(SettingSource::Local);
        }
        Some(raw) => {
            for token in raw.split(',') {
                match token.trim() {
                    "user" => {
                        set.insert(SettingSource::User);
                    }
                    "project" => {
                        set.insert(SettingSource::Project);
                    }
                    "local" => {
                        set.insert(SettingSource::Local);
                    }
                    "flag" => {
                        set.insert(SettingSource::Flag);
                    }
                    "policy" => {
                        set.insert(SettingSource::Policy);
                    }
                    _ => {}
                }
            }
        }
    }
    // Policy + Flag always participate.
    set.insert(SettingSource::Policy);
    set.insert(SettingSource::Flag);
    set
}

/// Build a runtime using the default `CatalogPaths` (the developer's
/// `config home/`). Test callers should prefer
/// `build_runtime_config_with` and pass a TempDir-rooted `CatalogPaths`
/// to avoid filesystem pollution.
pub fn build_runtime_config(
    settings: SettingsWithSource,
    env: EnvSnapshot,
    overrides: RuntimeOverrides,
) -> crate::Result<RuntimeConfig> {
    build_runtime_config_with(
        settings,
        env,
        overrides,
        CatalogPaths::default(),
        parse_enabled_setting_sources(None),
    )
}

/// Build a runtime with explicit catalog paths. Single-source for the
/// resolution pipeline; `build_runtime_config` is a thin wrapper that
/// uses the production defaults.
pub fn build_runtime_config_with(
    settings: SettingsWithSource,
    env: EnvSnapshot,
    overrides: RuntimeOverrides,
    catalogs: CatalogPaths,
    enabled_setting_sources: std::collections::HashSet<crate::settings::SettingSource>,
) -> crate::Result<RuntimeConfig> {
    let env_only = EnvOnlyConfig::from_snapshot(&env);
    let providers = resolve_providers(&settings, &catalogs)?;
    let model_roles = resolve_model_roles(&settings, &env_only, &overrides, &providers)?;
    let merged = &settings.merged;
    validate_roles_against_available_models(&model_roles, merged.available_models.as_deref())?;
    validate_moa_endpoints_against_available_models(
        &model_roles,
        merged.available_models.as_deref(),
    )?;

    let user_catalog = load_models_catalog(&catalogs.models)?;
    let model_registry = Arc::new(build_model_registry(
        &providers,
        &user_catalog,
        &catalogs.coco_home,
    )?);

    // Fail-fast: every role's primary + fallbacks must resolve through
    // the registry now, not later inside `build_api_client` where a
    // missing entry silently degrades to the legacy mock path (loses
    // `extra_body`, thinking translation, typed sampling). Surfaces
    // typos in `settings.models.<role>.{provider,model_id}` and
    // partial entries in `models.json` at startup.
    validate_roles_against_registry(&model_roles, &model_registry)?;
    validate_moa_endpoints_against_registry(&model_roles, &model_registry)?;

    let features = resolve_features(merged, &env, &overrides);
    let memory = MemoryConfig::try_resolve_with_sources(&settings, &env)?;
    let memory_activation = resolve_memory_activation(&features, &env, &memory);
    let tool_overrides = resolve_main_tool_overrides(&model_roles, &model_registry);
    // `skill_overrides` is read from per-tier raw JSON rather than
    // `merged` — see `SkillOverrideTiers` docs for why this field
    // sidesteps the standard deep-merge contract.
    let skill_overrides = SkillOverrideTiers::from_per_source(&settings.per_source);

    Ok(RuntimeConfig {
        api: ApiConfig::resolve(merged, &env),
        loop_config: LoopConfig::resolve(merged, &overrides, &env),
        tool: ToolConfig::resolve(merged, &env),
        shell: ShellConfig::resolve(merged, &env),
        output_rewrite: OutputRewriteConfig::resolve(merged, &env),
        sandbox: SandboxSettings::resolve(merged, &env),
        memory,
        memory_activation,
        mcp: McpRuntimeConfig::resolve(&settings, &env),
        web_fetch: WebFetchConfig::resolve(merged),
        web_search: WebSearchConfig::resolve(merged),
        diagnostics: DiagnosticsConfig::resolve(merged, &env),
        lsp: LspConfig::resolve(merged, &env),
        voice: VoiceConfig::resolve(merged, &env),
        event_hub: EventHubConfig::resolve(merged, &env, &overrides)?,
        server: ServerConfig::resolve(merged, &env),
        paths: PathConfig::resolve(merged),
        compact: CompactConfig::resolve(merged, &env),
        agent_teams: AgentTeamsConfig::resolve(merged)?,
        skill_learn: SkillLearnConfig::resolve(merged, &env),
        prompt_cache: PromptCacheRuntimeConfig::resolve(merged, &env),
        account: AccountConfig::resolve(merged, &env),
        features,
        skill_overrides,
        tool_overrides,
        enabled_setting_sources,
        settings,
        env_only,
        overrides,
        providers,
        model_roles,
        model_registry,
    })
}

fn validate_roles_against_available_models(
    roles: &ModelRoles,
    available_models: Option<&[String]>,
) -> Result<(), ConfigError> {
    let Some(available_models) = available_models else {
        return Ok(());
    };

    // `available_models` is a strict provider/model_id allowlist.
    // It does not imply fallback selection: any configured primary or
    // fallback outside the allowlist fails startup.
    for (role, slots) in &roles.roles {
        ensure_model_allowed(
            role.as_str(),
            "primary",
            &slots.primary.model,
            available_models,
        )?;
        for (idx, slot) in slots.fallbacks.iter().enumerate() {
            ensure_model_allowed(
                role.as_str(),
                &format!("fallbacks[{idx}]"),
                &slot.model,
                available_models,
            )?;
        }
    }
    Ok(())
}

fn validate_moa_endpoints_against_available_models(
    roles: &ModelRoles,
    available_models: Option<&[String]>,
) -> Result<(), ConfigError> {
    let Some(available_models) = available_models else {
        return Ok(());
    };

    for (preset_name, endpoint) in &roles.moa_presets {
        ensure_model_allowed(
            preset_name,
            "moa.aggregator",
            &endpoint.aggregator,
            available_models,
        )?;
        for (idx, spec) in endpoint.reference_models.iter().enumerate() {
            ensure_model_allowed(
                preset_name,
                &format!("moa.reference_models[{idx}]"),
                spec,
                available_models,
            )?;
        }
    }
    Ok(())
}

fn ensure_model_allowed(
    role: &str,
    slot: &str,
    spec: &ModelSpec,
    available_models: &[String],
) -> Result<(), ConfigError> {
    if is_model_allowed(&spec.provider, &spec.model_id, Some(available_models)) {
        return Ok(());
    }

    Err(ConfigError::generic(format!(
        "model `{}/{}` for role `{role}` slot `{slot}` is not in the available_models allowlist",
        spec.provider, spec.model_id,
    )))
}

/// Walk every (role, primary + fallback) `ModelSpec` and verify it
/// resolves in the registry. Surfaces both `IncompleteModelEntry`
/// (partial `models.json` entries) and `UnknownModel` (typos) at
/// config-build time instead of letting them silently disable
/// Layer-2 runtime plumbing.
fn validate_roles_against_registry(
    roles: &ModelRoles,
    registry: &ModelRegistry,
) -> Result<(), ConfigError> {
    for slots in roles.roles.values() {
        for slot in std::iter::once(&slots.primary).chain(slots.fallbacks.iter()) {
            let spec = &slot.model;
            match registry.try_resolve(&spec.provider, &spec.model_id)? {
                Some(_) => {}
                None => {
                    return Err(ConfigError::UnknownModel {
                        provider: spec.provider.clone(),
                        model: spec.model_id.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_moa_endpoints_against_registry(
    roles: &ModelRoles,
    registry: &ModelRegistry,
) -> Result<(), ConfigError> {
    for endpoint in roles.moa_presets.values() {
        for spec in std::iter::once(&endpoint.aggregator).chain(endpoint.reference_models.iter()) {
            match registry.try_resolve(&spec.provider, &spec.model_id)? {
                Some(_) => {}
                None => {
                    return Err(ConfigError::UnknownModel {
                        provider: spec.provider.clone(),
                        model: spec.model_id.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Resolve `tool_overrides` from the Main role's (provider, model_id)
/// via `ModelRegistry`.
fn resolve_main_tool_overrides(roles: &ModelRoles, registry: &ModelRegistry) -> Arc<ToolOverrides> {
    // Tool-overrides resolution keys on `model_id` alone — provider is
    // a routing concern (URL, auth, wire API), not a capability axis.
    // gpt-5 served by OpenAI direct, Azure, or any compat gateway
    // returns the same diff. The registry lookup additionally lets the
    // user-side `ModelInfo.tool_overrides` (settings.json) layer onto
    // the built-in diff.
    let Some(spec) = roles.get(ModelRole::Main) else {
        return Arc::new(ToolOverrides::none());
    };
    let info = registry
        .resolve(&spec.provider, &spec.model_id)
        .map(|r| r.info.clone());
    Arc::new(crate::tool_overrides::resolve_tool_overrides(
        &spec.model_id,
        info.as_ref(),
    ))
}

fn resolve_features(
    settings: &crate::settings::Settings,
    env: &EnvSnapshot,
    overrides: &RuntimeOverrides,
) -> Features {
    let mut features = Features::with_defaults();
    features.apply_map(&settings.features);
    features.apply_map(env.feature_overrides());
    for (feat, enabled) in &overrides.feature_overrides {
        features.set_enabled(*feat, *enabled);
    }
    features
}

fn resolve_memory_activation(
    features: &Features,
    env: &EnvSnapshot,
    memory: &MemoryConfig,
) -> MemoryActivation {
    if !features.enabled(coco_types::Feature::AutoMemory) {
        return MemoryActivation::disabled(MemoryDisabledReason::FeatureGate);
    }
    if env.is_truthy(crate::env::EnvKey::CocoBareMode) {
        return MemoryActivation::disabled(MemoryDisabledReason::BareMode);
    }
    if env.is_truthy(crate::env::EnvKey::CocoRemote)
        && memory.memory_base_override.is_none()
        && memory.directory.is_none()
    {
        return MemoryActivation::disabled(MemoryDisabledReason::RemoteWithoutMemoryDir);
    }
    MemoryActivation::active()
}

/// Load a JSON catalog file into a partial overlay map. Missing file
/// is not an error (returns empty); read or parse failures surface
/// typed errors so misconfiguration is visible at startup rather than
/// masking as "no entries".
fn load_models_catalog(
    path: &std::path::Path,
) -> Result<BTreeMap<String, PartialModelInfo>, ConfigError> {
    load_catalog_file(path)
}

fn load_providers_catalog(
    path: &std::path::Path,
) -> Result<BTreeMap<String, PartialProviderConfig>, ConfigError> {
    load_catalog_file(path)
}

fn load_catalog_file<T>(path: &std::path::Path) -> Result<BTreeMap<String, T>, ConfigError>
where
    T: serde::de::DeserializeOwned,
{
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => {
            return Err(ConfigError::CatalogRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    crate::jsonc::from_str(&contents).map_err(|source| ConfigError::CatalogParse {
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

fn resolve_providers(
    settings: &SettingsWithSource,
    catalogs: &CatalogPaths,
) -> Result<BTreeMap<String, ProviderConfig>, ConfigError> {
    let mut providers: BTreeMap<String, ProviderConfig> = BTreeMap::new();
    for provider in builtin_providers()? {
        providers.insert(provider.name.clone(), provider);
    }

    // L1: config home/providers.json (shared catalog)
    let file_catalog = load_providers_catalog(&catalogs.providers)?;
    apply_partial_layer(&mut providers, &file_catalog)?;

    // L2: settings.providers per-user overlay
    apply_partial_layer(&mut providers, &settings.merged.providers)?;
    Ok(providers)
}

fn apply_partial_layer(
    base: &mut BTreeMap<String, ProviderConfig>,
    overlay: &BTreeMap<String, PartialProviderConfig>,
) -> Result<(), ConfigError> {
    for (name, partial) in overlay {
        match base.get_mut(name) {
            Some(existing) => existing.merge_partial(partial)?,
            None => {
                let resolved = ProviderConfig::from_partial(name, partial)?;
                base.insert(name.clone(), resolved);
            }
        }
    }
    Ok(())
}

fn resolve_model_roles(
    settings: &SettingsWithSource,
    env: &EnvOnlyConfig,
    overrides: &RuntimeOverrides,
    providers: &BTreeMap<String, ProviderConfig>,
) -> crate::Result<ModelRoles> {
    let mut roles = ModelRoles::default();
    let moa_settings = resolve_moa_settings(
        &settings.merged.moa,
        &settings.merged.models,
        env,
        overrides,
    )?;
    validate_moa_settings(&moa_settings)?;
    for (preset_name, preset) in &moa_settings.presets {
        let endpoint =
            resolve_configured_moa_endpoint(ModelRole::Main, preset_name, preset, providers)?;
        roles.moa_presets.insert(preset_name.clone(), endpoint);
    }

    // Main resolution precedence: CLI override > env override >
    // settings.models.main. No silent fallback —
    // this is a multi-provider SDK; the user MUST pick a model.
    // Surfacing this as a startup error (instead of defaulting to a
    // built-in like `anthropic/claude-sonnet-4-6`) keeps the choice
    // of provider explicit and avoids charging the wrong account
    // when an unconfigured deployment ships.
    let mut main_slots = if let Some(selection) = overrides.model_override.as_ref() {
        RoleSlots::new(resolve_model_selection(selection.clone(), providers)?)
    } else if let Some(selection) = env.model_override.as_deref() {
        RoleSlots::new(model_spec_from_selection(selection, providers)?)
    } else if let Some(slots) = settings.merged.models.main.clone() {
        let (slots, endpoint) =
            resolve_role_slots(ModelRole::Main, slots, providers, &moa_settings)?;
        if let Some(endpoint) = endpoint {
            roles.moa_endpoints.insert(ModelRole::Main, endpoint);
        }
        slots
    } else {
        return Err(crate::ConfigError::generic(
            "no Main model configured: set `models.main` in settings.json, \
             pass `--models.main <provider>/<model_id>`, or set \
             `COCO_MODEL_MAIN=<provider>/<model_id>`",
        ));
    };

    // CLI `--fallback-model` overrides settings.json fallbacks for
    // Main. Resolving here (after `main_slots` is built) means the
    // CLI can swap in a completely different chain even when
    // settings.models.main.fallbacks is populated.
    if !overrides.fallback_model_overrides.is_empty() {
        let fallbacks: Vec<coco_types::ModelSpec> = overrides
            .fallback_model_overrides
            .iter()
            .map(|sel| resolve_model_selection(sel.clone(), providers))
            .collect::<crate::Result<Vec<_>>>()?;
        // CLI `--fallback-model` carries no per-slot effort — wrap each
        // as a bare slot (effort defers to the model default).
        main_slots = RoleSlots {
            primary: main_slots.primary,
            fallbacks: fallbacks.into_iter().map(RoleSlot::bare).collect(),
            policy: main_slots.policy,
        };
        ensure_chain_unique(ModelRole::Main, &main_slots)?;
    }

    roles.roles.insert(ModelRole::Main, main_slots);

    set_role_from_json(
        &mut roles,
        ModelRole::Fast,
        settings.merged.models.fast.as_ref(),
        providers,
        &moa_settings,
    )?;
    set_role_from_json(
        &mut roles,
        ModelRole::Plan,
        settings.merged.models.plan.as_ref(),
        providers,
        &moa_settings,
    )?;
    set_role_from_json(
        &mut roles,
        ModelRole::Explore,
        settings.merged.models.explore.as_ref(),
        providers,
        &moa_settings,
    )?;
    set_role_from_json(
        &mut roles,
        ModelRole::Review,
        settings.merged.models.review.as_ref(),
        providers,
        &moa_settings,
    )?;
    set_role_from_json(
        &mut roles,
        ModelRole::HookAgent,
        settings.merged.models.hook_agent.as_ref(),
        providers,
        &moa_settings,
    )?;
    set_role_from_json(
        &mut roles,
        ModelRole::Memory,
        settings.merged.models.memory.as_ref(),
        providers,
        &moa_settings,
    )?;
    set_role_from_json(
        &mut roles,
        ModelRole::Subagent,
        settings.merged.models.subagent.as_ref(),
        providers,
        &moa_settings,
    )?;

    // Default unconfigured roles to Main. This makes the consumer side
    // (`runtime_config.model_roles.get(role)`) always return `Some(spec)`
    // so subagents / hook-agent / compact summarizer / session-memory
    // extractor / etc. don't each need their own `or_else` fallback chain.
    //
    // Single source of truth: every role goes through settings.json
    // (`models.<role>`). The legacy `COCO_SMALL_FAST_MODEL` /
    // `COCO_SUBAGENT_MODEL` env-only overrides have been removed —
    // configure via settings instead. Only `COCO_MODEL_MAIN` survives as
    // the single-knob Main escape hatch (handled above).
    if let Some(main_slots) = roles.roles.get(&ModelRole::Main).cloned() {
        let mut defaulted_roles = Vec::new();
        for fallback_role in ModelRole::ALL {
            if let std::collections::hash_map::Entry::Vacant(e) = roles.roles.entry(fallback_role) {
                // Borrow Main's models but NOT its effort — an
                // unconfigured role has declared no thinking intent, so
                // each defaulted slot defers to the model default. Effort
                // isolation: `models.main.effort` never leaks into a role
                // the user didn't configure.
                e.insert(main_slots.without_effort());
                if let Some(endpoint) = roles.moa_endpoints.get(&ModelRole::Main).cloned() {
                    roles.moa_endpoints.insert(fallback_role, endpoint);
                }
                defaulted_roles.push(fallback_role.as_str());
            }
        }
        if !defaulted_roles.is_empty() {
            tracing::debug!(
                roles = %defaulted_roles.join(","),
                main_model = %main_slots.primary.model.model_id,
                "model roles unconfigured; defaulting to Main",
            );
        }
    }

    // Invariant consumed downstream: after this pass every role has an
    // entry (Main is mandatory above; the loop fills the rest). The
    // model-runtime registry reads `ModelRoles::primary_slot` — which
    // deliberately does NOT fall back to Main, so effort can't leak
    // across roles — and relies on this pass having populated the map.
    debug_assert!(
        ModelRole::ALL
            .iter()
            .all(|role| roles.roles.contains_key(role)),
        "build_model_roles must leave every ModelRole populated",
    );

    Ok(roles)
}

fn resolve_moa_settings(
    settings: &MoaSettings,
    models: &crate::ModelSelectionSettings,
    env: &EnvOnlyConfig,
    overrides: &RuntimeOverrides,
) -> crate::Result<MoaSettings> {
    let mut resolved = settings.clone();
    if should_synthesize_default_moa_preset(settings, models, env, overrides) {
        let preset = synthesize_default_moa_preset(models, env, overrides)?;
        resolved.presets.insert("default".to_string(), preset);
        if resolved.default_preset.is_none() {
            resolved.default_preset = Some("default".to_string());
        }
    }
    Ok(resolved)
}

fn should_synthesize_default_moa_preset(
    settings: &MoaSettings,
    models: &crate::ModelSelectionSettings,
    env: &EnvOnlyConfig,
    overrides: &RuntimeOverrides,
) -> bool {
    if settings.presets.contains_key("default") {
        return false;
    }
    if settings
        .default_preset
        .as_deref()
        .is_some_and(|preset| preset != "default")
    {
        return false;
    }
    if settings.default_preset.as_deref() == Some("default")
        || model_settings_select_moa_preset(models, "default")
    {
        return true;
    }

    can_synthesize_default_moa_preset(models, env, overrides)
}

fn model_settings_select_moa_preset(
    models: &crate::ModelSelectionSettings,
    preset_name: &str,
) -> bool {
    [
        models.main.as_ref(),
        models.fast.as_ref(),
        models.plan.as_ref(),
        models.explore.as_ref(),
        models.review.as_ref(),
        models.hook_agent.as_ref(),
        models.memory.as_ref(),
        models.subagent.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|slots| {
        slots.primary.model.provider == MOA_PROVIDER && slots.primary.model.model_id == preset_name
    })
}

fn synthesize_default_moa_preset(
    models: &crate::ModelSelectionSettings,
    env: &EnvOnlyConfig,
    overrides: &RuntimeOverrides,
) -> crate::Result<MoaPresetSettings> {
    let aggregator = real_main_selection(models, env, overrides)?;
    if aggregator.provider == MOA_PROVIDER {
        return Err(crate::ConfigError::generic(
            "cannot synthesize settings.moa.presets.default from a MoA Main model; configure the preset aggregator and reference_models explicitly",
        ));
    }

    let mut seen = std::collections::HashSet::new();
    seen.insert((aggregator.provider.clone(), aggregator.model_id.clone()));
    let mut reference_models = Vec::new();
    for slots in [models.review.as_ref(), models.fast.as_ref()]
        .into_iter()
        .flatten()
    {
        let selection = &slots.primary.model;
        if selection.provider == MOA_PROVIDER {
            continue;
        }
        if seen.insert((selection.provider.clone(), selection.model_id.clone())) {
            reference_models.push(selection.clone());
        }
    }

    if reference_models.is_empty() {
        return Err(crate::ConfigError::generic(
            "cannot synthesize settings.moa.presets.default: configure at least one real, non-duplicate models.review or models.fast reference model",
        ));
    }

    Ok(MoaPresetSettings {
        aggregator: Some(aggregator),
        reference_models,
        ..Default::default()
    })
}

fn can_synthesize_default_moa_preset(
    models: &crate::ModelSelectionSettings,
    env: &EnvOnlyConfig,
    overrides: &RuntimeOverrides,
) -> bool {
    let Ok(aggregator) = real_main_selection(models, env, overrides) else {
        return false;
    };
    if aggregator.provider == MOA_PROVIDER {
        return false;
    }

    let mut seen = std::collections::HashSet::new();
    seen.insert((aggregator.provider, aggregator.model_id));
    [models.review.as_ref(), models.fast.as_ref()]
        .into_iter()
        .flatten()
        .any(|slots| {
            let selection = &slots.primary.model;
            selection.provider != MOA_PROVIDER
                && seen.insert((selection.provider.clone(), selection.model_id.clone()))
        })
}

fn real_main_selection(
    models: &crate::ModelSelectionSettings,
    env: &EnvOnlyConfig,
    overrides: &RuntimeOverrides,
) -> crate::Result<ProviderModelSelection> {
    if let Some(selection) = overrides.model_override.as_ref() {
        return Ok(selection.clone());
    }
    if let Some(selection) = env.model_override.as_deref() {
        return ProviderModelSelection::from_slash_str(selection)
            .map_err(crate::ConfigError::generic);
    }
    models
        .main
        .as_ref()
        .map(|slots| slots.primary.model.clone())
        .ok_or_else(|| {
            crate::ConfigError::generic(
                "cannot synthesize settings.moa.presets.default without a real Main model",
            )
        })
}

fn set_role_from_json(
    roles: &mut ModelRoles,
    role: ModelRole,
    slots: Option<&RoleSlots<ProviderModelSelection>>,
    providers: &BTreeMap<String, ProviderConfig>,
    moa_settings: &MoaSettings,
) -> crate::Result<()> {
    if let Some(slots) = slots {
        let (slots, endpoint) = resolve_role_slots(role, slots.clone(), providers, moa_settings)?;
        roles.roles.insert(role, slots);
        if let Some(endpoint) = endpoint {
            roles.moa_endpoints.insert(role, endpoint);
        } else {
            roles.moa_endpoints.remove(&role);
        }
    }
    Ok(())
}

fn resolve_role_slots(
    role: ModelRole,
    slots: RoleSlots<ProviderModelSelection>,
    providers: &BTreeMap<String, ProviderConfig>,
    moa_settings: &MoaSettings,
) -> crate::Result<(RoleSlots<ModelSpec>, Option<MoaEndpointSpec>)> {
    let mut endpoint = None;
    let primary_effort = slots.primary.effort;
    let primary = if moa::is_moa_selection(&slots.primary.model) {
        let resolved_endpoint =
            resolve_moa_endpoint(role, &slots.primary.model.model_id, providers, moa_settings)?;
        let aggregator = resolved_endpoint.aggregator.clone();
        endpoint = Some(resolved_endpoint);
        RoleSlot {
            model: aggregator,
            effort: primary_effort,
        }
    } else {
        RoleSlot {
            model: resolve_model_selection(slots.primary.model, providers)?,
            effort: primary_effort,
        }
    };
    let fallbacks = slots
        .fallbacks
        .into_iter()
        .enumerate()
        .map(|(idx, slot)| {
            if moa::is_moa_selection(&slot.model) {
                return Err(crate::ConfigError::generic(format!(
                    "role `{role:?}` fallback[{idx}] uses `{MOA_PROVIDER}/{}`; MoA is only supported as a role primary",
                    slot.model.model_id
                )));
            }
            Ok(RoleSlot {
                model: resolve_model_selection(slot.model, providers)?,
                effort: slot.effort,
            })
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let resolved = RoleSlots {
        primary,
        fallbacks,
        policy: slots.policy,
    };
    ensure_chain_unique(role, &resolved)?;
    Ok((resolved, endpoint))
}

fn validate_moa_settings(settings: &MoaSettings) -> crate::Result<()> {
    for name in settings.presets.keys() {
        if name.trim().is_empty() {
            return Err(crate::ConfigError::generic(
                "settings.moa preset name must be non-empty",
            ));
        }
    }
    if let Some(default_preset) = settings.default_preset.as_deref() {
        let preset = settings.presets.get(default_preset).ok_or_else(|| {
            crate::ConfigError::generic(format!(
                "settings.moa.default_preset `{default_preset}` does not exist"
            ))
        })?;
        if !preset.enabled {
            return Err(crate::ConfigError::generic(format!(
                "settings.moa.default_preset `{default_preset}` is disabled"
            )));
        }
    }
    Ok(())
}

fn resolve_moa_endpoint(
    role: ModelRole,
    preset_name: &str,
    providers: &BTreeMap<String, ProviderConfig>,
    settings: &MoaSettings,
) -> crate::Result<MoaEndpointSpec> {
    if preset_name.trim().is_empty() {
        return Err(crate::ConfigError::generic(format!(
            "role `{role:?}` selects `{MOA_PROVIDER}/` with an empty preset name"
        )));
    }
    let preset = settings.presets.get(preset_name).ok_or_else(|| {
        crate::ConfigError::generic(format!(
            "role `{role:?}` selects `{MOA_PROVIDER}/{preset_name}`, but settings.moa.presets.{preset_name} is not configured"
        ))
    })?;
    resolve_configured_moa_endpoint(role, preset_name, preset, providers)
}

fn resolve_configured_moa_endpoint(
    role: ModelRole,
    preset_name: &str,
    preset: &MoaPresetSettings,
    providers: &BTreeMap<String, ProviderConfig>,
) -> crate::Result<MoaEndpointSpec> {
    if !preset.enabled {
        return Err(crate::ConfigError::generic(format!(
            "role `{role:?}` selects disabled MoA preset `{preset_name}`"
        )));
    }
    let aggregator = preset.aggregator.clone().ok_or_else(|| {
        crate::ConfigError::generic(format!(
            "MoA preset `{preset_name}` must configure an aggregator provider/model"
        ))
    })?;
    ensure_real_moa_member(preset_name, "aggregator", &aggregator)?;
    let aggregator = resolve_model_selection(aggregator, providers)?;

    let reference_models = preset
        .reference_models
        .iter()
        .enumerate()
        .map(|(idx, selection)| {
            ensure_real_moa_member(preset_name, &format!("reference_models[{idx}]"), selection)?;
            resolve_model_selection(selection.clone(), providers)
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let reference_models = moa::dedupe_specs(reference_models);
    if reference_models.is_empty() {
        return Err(crate::ConfigError::generic(format!(
            "MoA preset `{preset_name}` must configure at least one reference model"
        )));
    }
    if reference_models.len() > MAX_REFERENCE_MODELS {
        return Err(crate::ConfigError::generic(format!(
            "MoA preset `{preset_name}` has {} reference models after dedupe; maximum is {MAX_REFERENCE_MODELS}",
            reference_models.len()
        )));
    }

    Ok(MoaEndpointSpec {
        preset_name: preset_name.to_string(),
        aggregator,
        reference_models,
        fanout: preset.fanout,
        reference_max_tokens: preset.reference_max_tokens,
        reference_temperature: preset.reference_temperature,
        aggregator_temperature: preset.aggregator_temperature,
    })
}

fn ensure_real_moa_member(
    preset_name: &str,
    field: &str,
    selection: &ProviderModelSelection,
) -> crate::Result<()> {
    if selection.provider == MOA_PROVIDER {
        return Err(crate::ConfigError::generic(format!(
            "MoA preset `{preset_name}` field `{field}` must use a real provider/model, not `{MOA_PROVIDER}/{}`",
            selection.model_id
        )));
    }
    Ok(())
}

fn ensure_chain_unique(role: ModelRole, slots: &RoleSlots<ModelSpec>) -> crate::Result<()> {
    let mut seen: HashMap<(String, String), &'static str> = HashMap::new();
    seen.insert(
        (
            slots.primary.model.provider.clone(),
            slots.primary.model.model_id.clone(),
        ),
        "primary",
    );
    for (idx, fb) in slots.fallbacks.iter().enumerate() {
        let key = (fb.model.provider.clone(), fb.model.model_id.clone());
        if let Some(prev) = seen.get(&key) {
            return Err(crate::ConfigError::generic(format!(
                "role `{role:?}`: fallback[{idx}] `{}/{}` duplicates {prev} slot; \
                 each slot in the chain must be a distinct model",
                fb.model.provider, fb.model.model_id,
            )));
        }
        seen.insert(key, "earlier fallback");
    }
    Ok(())
}

fn model_spec_from_selection(
    selection: &str,
    providers: &BTreeMap<String, ProviderConfig>,
) -> crate::Result<ModelSpec> {
    let (provider_name, model_id) = selection.split_once('/').ok_or_else(|| {
        crate::ConfigError::generic(format!(
            "model selection `{selection}` must use explicit `provider/model_id` format"
        ))
    })?;
    if provider_name.is_empty() || model_id.is_empty() {
        return Err(crate::ConfigError::generic(format!(
            "model selection `{selection}` must use explicit `provider/model_id` format"
        )));
    }
    let provider = providers.get(provider_name).ok_or_else(|| {
        crate::ConfigError::generic(format!(
            "model selection `{selection}` references unknown provider `{provider_name}`"
        ))
    })?;
    Ok(ModelSpec {
        provider: provider_name.to_string(),
        api: provider.api,
        model_id: model_id.to_string(),
        display_name: model_id.to_string(),
    })
}

fn resolve_model_selection(
    selection: ProviderModelSelection,
    providers: &BTreeMap<String, ProviderConfig>,
) -> crate::Result<ModelSpec> {
    if selection.provider.is_empty() || selection.model_id.is_empty() {
        return Err(crate::ConfigError::generic(
            "model role selection must include non-empty `provider` and `model_id`",
        ));
    }
    let provider = providers.get(&selection.provider).ok_or_else(|| {
        crate::ConfigError::generic(format!(
            "model `{}` references unknown provider `{}`",
            selection.model_id, selection.provider
        ))
    })?;
    Ok(ModelSpec {
        provider: selection.provider,
        api: provider.api,
        display_name: selection.model_id.clone(),
        model_id: selection.model_id,
    })
}

/// Hot-reload publisher.
///
/// Wraps `tokio::sync::watch` so subscribers can grab the latest
/// `Arc<RuntimeConfig>` snapshot at turn boundaries without locking.
/// In-flight turns retain the `Arc` they captured at turn start; a
/// mid-turn config change has no torn-read effect — the next turn
/// picks up the new snapshot atomically.
#[derive(Debug, Clone)]
pub struct RuntimePublisher {
    sender: tokio::sync::watch::Sender<Arc<RuntimeConfig>>,
}

impl RuntimePublisher {
    pub fn new(initial: Arc<RuntimeConfig>) -> Self {
        let (sender, _) = tokio::sync::watch::channel(initial);
        Self { sender }
    }

    /// Publish a fresh snapshot. Subscribers see the new `Arc`
    /// at their next `borrow().clone()`.
    ///
    /// Uses `send_replace` rather than `send`: the latter no-ops and
    /// returns `Err` when there are zero live receivers (and `new`
    /// drops its own receiver), which would silently leave `current()`
    /// returning the stale snapshot. `send_replace` always stores the
    /// value and still wakes any subscribers.
    pub fn publish(&self, runtime: Arc<RuntimeConfig>) {
        self.sender.send_replace(runtime);
    }

    /// Subscribe to runtime updates. Each subscriber gets its own
    /// receiver; cloning is cheap (`watch::Receiver` is Arc-internally).
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Arc<RuntimeConfig>> {
        self.sender.subscribe()
    }

    /// Borrow the current snapshot without subscribing.
    pub fn current(&self) -> Arc<RuntimeConfig> {
        self.sender.borrow().clone()
    }
}

#[cfg(test)]
#[path = "runtime.test.rs"]
mod tests;
