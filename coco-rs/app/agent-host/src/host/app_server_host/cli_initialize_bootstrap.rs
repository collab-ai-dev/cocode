//! CLI-side [`InitializeBootstrap`] implementation.
//!
//! Concentrates the cross-subsystem data sources for the `initialize`
//! wire response so every source (commands, output styles, agents,
//! auth) lives in one place rather than spraying 5+ fields across
//! `AppServerHostState`. The server holds an `Arc<dyn InitializeBootstrap>`
//! trait object; the concrete impl below knows about every subsystem
//! and wires them together at CLI startup.
//!
//! Fields that require richer cross-crate plumbing (agent discovery,
//! provider auth exposure) return stub values today and will be
//! filled in as their data sources grow an accessor.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use coco_commands::CommandRegistry;
use coco_config::RuntimeConfig;
use coco_config::global_config;
use coco_inference::auth::AuthMethod;
use coco_subagent::AgentDefinitionStore;
use coco_subagent::BuiltinAgentCatalog;
use coco_subagent::definition_store::AgentSearchPaths;
use coco_types::AgentDefinition;
use coco_types::FastModeState;

use crate::app_server_host::InitializeBootstrap;
use crate::session_bootstrap::EngineResources;
use crate::session_runtime::SessionAccountProvider;
use crate::session_runtime::SessionInitializeAccount;
use crate::session_runtime::SessionInitializeAgent;
use crate::session_runtime::SessionInitializeCommand;

/// Built-in output style names shipped with coco-rs. Uses a lowercase
/// `"default"` sentinel plus capitalized `"Explanatory"` and `"Learning"`.
/// Case matters: clients looking up a style by name do an exact-string
/// match. Used as the fallback when no manager is wired.
pub const BUILTIN_OUTPUT_STYLES: &[&str] = &[
    coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME,
    coco_output_styles::EXPLANATORY_STYLE_NAME,
    coco_output_styles::LEARNING_STYLE_NAME,
];

/// Concrete [`InitializeBootstrap`] wired from CLI startup.
///
/// Holds `Arc` references to the data sources so the trait object can
/// be cheaply shared between AppServer host state and any future consumers.
/// Each accessor reads from its paired field — missing sources return
/// empty / default values instead of erroring so `initialize` is always
/// a successful handshake.
pub struct CliInitializeBootstrap {
    /// Slash-command registry populated at CLI startup (built-ins +
    /// plugin + user markdown). `None` disables `commands`. Wrapped in
    /// `RwLock<Arc<...>>` so reloads (`/reload-plugins`) are observed
    /// by subsequent `initialize` calls without rebuilding the
    /// bootstrap.
    pub command_registry: Option<Arc<tokio::sync::RwLock<Arc<CommandRegistry>>>>,
    /// Resolved active output style name. Defaults to `"default"` and
    /// reflects the name exposed through the initialize response.
    pub output_style: String,
    /// All output style names advertised to initialize clients as selectable
    /// (`available_output_styles`). The CLI seeds this from
    /// [`coco_output_styles::OutputStyleManager::names`] and prepends
    /// the `default` sentinel.
    pub available_styles: Vec<String>,
    /// Search paths for custom agent definition markdown files. Built-ins
    /// resolved through [`coco_subagent::BuiltinAgentCatalog::interactive`]
    /// are always included on top.
    pub agent_search_paths: AgentSearchPaths,
    /// Workspace cwd captured by the CLI bootstrap path. Used for
    /// initialize-time agent snapshot inspection before a session exists.
    pub cwd: std::path::PathBuf,
    /// Resolved auth method for the active session. `None` means no auth
    /// configured, so `account()` returns empty account metadata.
    pub auth_method: Option<Arc<AuthMethod>>,
}

impl CliInitializeBootstrap {
    /// Construct a new provider with only the output style wired.
    /// Other sources default to empty until explicitly set.
    pub fn new(output_style: String) -> Self {
        Self {
            command_registry: None,
            output_style,
            available_styles: BUILTIN_OUTPUT_STYLES.iter().map(|s| (*s).into()).collect(),
            agent_search_paths: AgentSearchPaths::empty(),
            cwd: std::path::PathBuf::from("."),
            auth_method: None,
        }
    }

    pub fn with_cwd(mut self, cwd: std::path::PathBuf) -> Self {
        self.cwd = cwd;
        self
    }

    pub fn with_command_registry(
        mut self,
        registry: Arc<tokio::sync::RwLock<Arc<CommandRegistry>>>,
    ) -> Self {
        self.command_registry = Some(registry);
        self
    }

    /// Override the initialize-advertised output style name list. The CLI
    /// builds this from the resolved `OutputStyleManager` — the wire
    /// list includes built-ins (`Explanatory`, `Learning`) plus any
    /// custom dir / plugin styles, with the `default` sentinel prepended.
    pub fn with_available_output_styles(mut self, styles: Vec<String>) -> Self {
        self.available_styles = styles;
        self
    }

    pub fn with_agent_search_paths(mut self, paths: AgentSearchPaths) -> Self {
        self.agent_search_paths = paths;
        self
    }

    pub fn with_auth_method(mut self, auth: AuthMethod) -> Self {
        self.auth_method = Some(Arc::new(auth));
        self
    }
}

pub(crate) async fn build_remote_initialize_bootstrap(
    resources: &EngineResources,
    runtime_config: &RuntimeConfig,
    cwd: &Path,
) -> Arc<dyn InitializeBootstrap> {
    let current_output_style = resources.output_style_manager.active_name_for_initialize();
    let mut available_output_styles = resources.output_style_manager.names();
    if !available_output_styles
        .iter()
        .any(|n| n == coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME)
    {
        available_output_styles
            .insert(0, coco_output_styles::DEFAULT_OUTPUT_STYLE_NAME.to_string());
    }
    let agent_search_paths = resources
        .project_services
        .agent_search_paths(&global_config::config_home(), cwd);
    let auth_method = remote_auth_method(resources, runtime_config).await;

    let mut bootstrap = CliInitializeBootstrap::new(current_output_style)
        .with_cwd(cwd.to_path_buf())
        .with_command_registry(resources.command_registry.clone())
        .with_available_output_styles(available_output_styles)
        .with_agent_search_paths(agent_search_paths);
    if let Some(auth) = auth_method {
        bootstrap = bootstrap.with_auth_method(auth);
    }
    Arc::new(bootstrap)
}

async fn remote_auth_method(
    resources: &EngineResources,
    runtime_config: &RuntimeConfig,
) -> Option<AuthMethod> {
    if resources.provider_api != Some(coco_types::ProviderApi::Anthropic) {
        return None;
    }
    let config_dir = global_config::config_home();
    // Deliberately not `settings.merged`: this string is executed via `sh -c`,
    // and the merged view would let a cloned repository's `.cocode/settings.json`
    // supply it. See `SettingsWithSource::api_key_helper`.
    let api_key_helper = runtime_config.settings.api_key_helper();
    let force_env_auth = runtime_config.env_only.force_env_auth;
    tokio::task::spawn_blocking(move || {
        coco_inference::auth::resolve_auth(&coco_inference::auth::AuthResolveOptions {
            config_dir: Some(config_dir),
            api_key_helper,
            force_env_auth,
            ..Default::default()
        })
    })
    .await
    .ok()
    .flatten()
}

#[async_trait]
impl InitializeBootstrap for CliInitializeBootstrap {
    async fn commands(&self) -> Vec<SessionInitializeCommand> {
        let Some(slot) = self.command_registry.as_ref() else {
            return Vec::new();
        };
        // Snapshot once — a concurrent reload swaps the inner Arc but
        // the snapshot stays valid for the duration of this call.
        let registry = slot.read().await.clone();
        // `client_visible()` is strictly tighter than `visible()`: it also
        // filters `is_sensitive` so external clients never see
        // command names / descriptions / argument hints for commands
        // flagged as sensitive (even though local TUI completions
        // may show them).
        registry
            .remote_client_visible()
            .iter()
            .map(|cmd| SessionInitializeCommand {
                name: cmd.base.name.clone(),
                description: cmd.base.description.clone(),
                argument_hint: cmd.base.argument_hint.clone().unwrap_or_default(),
            })
            .collect()
    }

    async fn agents(&self) -> Vec<SessionInitializeAgent> {
        let paths = self.agent_search_paths.clone();
        // Decorate every loaded definition with its `pendingSnapshotUpdate`
        // timestamp so the initialize agent listing surfaces drift
        // to clients. The closure runs blocking IO inside the spawn_blocking
        // closure below, so its captured paths are owned `PathBuf`s.
        let cwd = self.cwd.clone();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        tokio::task::spawn_blocking(move || {
            let mut store = AgentDefinitionStore::new(BuiltinAgentCatalog::interactive(), paths);
            store.set_snapshot_inspector(Some(
                coco_memory::agent_memory_snapshot::build_pending_inspector(cwd, home),
            ));
            store.load();
            // The store already applies source precedence — built-ins under
            // user/project markdown overrides — so iterating `active()` gives
            // the deduplicated set the AgentTool will see at spawn time.
            let mut out: Vec<SessionInitializeAgent> = store
                .snapshot()
                .active()
                .cloned()
                .map(def_to_initialize_agent)
                .collect();
            out.sort_by(|a, b| a.name.cmp(&b.name));
            out
        })
        .await
        .unwrap_or_else(|_| {
            // spawn_blocking panicked inside the closure. Fall back to the
            // built-in set so `initialize.agents` is never empty just
            // because a markdown file had a parse bug.
            coco_subagent::builtin_definitions(BuiltinAgentCatalog::interactive())
                .into_iter()
                .map(def_to_initialize_agent)
                .collect()
        })
    }

    async fn account(&self) -> SessionInitializeAccount {
        match self.auth_method.as_deref() {
            Some(auth) => auth_method_to_account_metadata(auth),
            None => SessionInitializeAccount::default(),
        }
    }

    async fn output_style(&self) -> String {
        self.output_style.clone()
    }

    async fn available_output_styles(&self) -> Vec<String> {
        self.available_styles.clone()
    }

    async fn fast_mode_state(&self) -> Option<FastModeState> {
        // Runtime-backed initialize reads the live session state directly.
        // Before a runtime exists, this bootstrap has no rate-limit state to
        // advertise.
        None
    }

    async fn cwd(&self) -> std::path::PathBuf {
        self.cwd.clone()
    }
}

/// Shared projection from a coco-rs [`AgentDefinition`] to initialize metadata.
pub(crate) fn def_to_initialize_agent(def: AgentDefinition) -> SessionInitializeAgent {
    SessionInitializeAgent {
        name: def.name,
        description: def.description.unwrap_or_default(),
        model: def.model,
    }
}

/// Map a resolved [`AuthMethod`] to initialize account metadata.
///
/// - **Third-party providers** (Bedrock / Vertex / Foundry): returns an
///   empty account to indicate no first-party account info.
/// - **First-party OAuth**: `api_provider = FirstParty`,
///   `subscription_type` from the token, `organization` is the raw
///   `org_uuid` (human-readable name requires a separate API call we
///   don't make yet). `token_source` is intentionally `None` — the
///   known token-source strings don't map cleanly from the
///   `AuthMethod::OAuth` variant and sending an incompatible value
///   would mislead clients that key on those values. `email` is
///   always `None` (OAuth token doesn't embed it).
/// - **First-party API key**: `api_provider = FirstParty` only.
///   `api_key_source` stays `None` until coco-rs tracks the env var /
///   helper origin (`user` / `project` / `org` / `temporary` / `oauth`).
pub fn auth_method_to_account_metadata(auth: &AuthMethod) -> SessionInitializeAccount {
    match auth {
        AuthMethod::OAuth(tokens) => SessionInitializeAccount {
            email: None,
            organization: tokens.org_uuid.clone(),
            subscription_type: tokens.subscription_type.clone(),
            token_source: None,
            api_key_source: None,
            api_provider: Some(SessionAccountProvider::FirstParty),
        },
        AuthMethod::ApiKey { .. } => SessionInitializeAccount {
            api_provider: Some(SessionAccountProvider::FirstParty),
            ..Default::default()
        },
        // Third-party provider paths: return a bare default so consumers that
        // check `account.apiProvider === undefined` to detect 3P auth do not
        // treat the session as first-party "logged in".
        AuthMethod::Bedrock { .. } | AuthMethod::Vertex { .. } | AuthMethod::Foundry { .. } => {
            SessionInitializeAccount::default()
        }
    }
}

#[cfg(test)]
#[path = "cli_initialize_bootstrap.test.rs"]
mod tests;
