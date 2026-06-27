//! CLI-side [`InitializeBootstrap`] implementation.
//!
//! Concentrates the cross-subsystem data sources for the `initialize`
//! wire response so every source (commands, output styles, agents,
//! auth) lives in one place rather than spraying 5+ fields across
//! `SdkServerState`. The server holds an `Arc<dyn InitializeBootstrap>`
//! trait object; the concrete impl below knows about every subsystem
//! and wires them together at CLI startup.
//!
//! Fields that require richer cross-crate plumbing (agent discovery,
//! provider auth exposure) return stub values today and will be
//! filled in as their data sources grow an accessor.

use std::sync::Arc;

use async_trait::async_trait;
use coco_commands::CommandRegistry;
use coco_inference::auth::AuthMethod;
use coco_subagent::AgentDefinitionStore;
use coco_subagent::AgentDefinitionValidator;
use coco_subagent::BuiltinAgentCatalog;
use coco_subagent::definition_store::AgentSearchPaths;
use coco_types::AgentDefinition;
use coco_types::AgentSource;
use coco_types::AgentTypeId;
use coco_types::FastModeState;
use coco_types::SdkAccountInfo;
use coco_types::SdkAgentDefinition;
use coco_types::SdkAgentInfo;
use coco_types::SdkApiProvider;
use coco_types::SdkSlashCommand;
use coco_types::ToolAllowList;
use std::collections::HashMap;
use std::str::FromStr;

use crate::sdk_server::handlers::InitializeBootstrap;

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
/// be cheaply shared between `SdkServerState` and any future consumers.
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
    /// reflects [`coco_output_styles::OutputStyleManager::active_name_for_sdk`].
    pub output_style: String,
    /// All output style names the SDK should advertise as selectable
    /// (`available_output_styles`). The CLI seeds this from
    /// [`coco_output_styles::OutputStyleManager::names`] and prepends
    /// the `default` sentinel.
    pub available_styles: Vec<String>,
    /// Search paths for custom agent definition markdown files. Built-ins
    /// resolved through [`coco_subagent::BuiltinAgentCatalog::interactive`]
    /// are always included on top.
    pub agent_search_paths: AgentSearchPaths,
    /// Resolved auth method for the active session. Controls how
    /// `account()` maps to [`coco_types::SdkAccountInfo`]. `None` →
    /// no auth configured, empty account.
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
            auth_method: None,
        }
    }

    pub fn with_command_registry(
        mut self,
        registry: Arc<tokio::sync::RwLock<Arc<CommandRegistry>>>,
    ) -> Self {
        self.command_registry = Some(registry);
        self
    }

    /// Override the SDK-advertised output style name list. The CLI
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

#[async_trait]
impl InitializeBootstrap for CliInitializeBootstrap {
    async fn commands(&self) -> Vec<SdkSlashCommand> {
        let Some(slot) = self.command_registry.as_ref() else {
            return Vec::new();
        };
        // Snapshot once — a concurrent reload swaps the inner Arc but
        // the snapshot stays valid for the duration of this call.
        let registry = slot.read().await.clone();
        // `sdk_safe()` is strictly tighter than `visible()`: it also
        // filters `is_sensitive` so remote SDK clients never see
        // command names / descriptions / argument hints for commands
        // flagged as sensitive (even though local TUI completions
        // may show them).
        registry
            .sdk_safe()
            .iter()
            .map(|cmd| SdkSlashCommand {
                name: cmd.base.name.clone(),
                description: cmd.base.description.clone(),
                // `argumentHint` is REQUIRED (not optional). When
                // coco-rs has no hint, we advertise an empty string so
                // strict parsers accept the response.
                argument_hint: cmd.base.argument_hint.clone().unwrap_or_default(),
            })
            .collect()
    }

    async fn agents(&self) -> Vec<SdkAgentInfo> {
        let paths = self.agent_search_paths.clone();
        // Decorate every loaded definition with its `pendingSnapshotUpdate`
        // timestamp so the SDK's `initialize.agents` listing surfaces drift
        // to clients. The closure runs blocking IO inside the spawn_blocking
        // closure below, so its captured paths are owned `PathBuf`s.
        let cwd = std::env::current_dir().unwrap_or_default();
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
            let mut out: Vec<SdkAgentInfo> = store
                .snapshot()
                .active()
                .cloned()
                .map(def_to_sdk_agent_info)
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
                .map(def_to_sdk_agent_info)
                .collect()
        })
    }

    async fn account(&self) -> SdkAccountInfo {
        match self.auth_method.as_deref() {
            Some(auth) => auth_method_to_account(auth),
            None => SdkAccountInfo::default(),
        }
    }

    async fn output_style(&self) -> String {
        self.output_style.clone()
    }

    async fn available_output_styles(&self) -> Vec<String> {
        self.available_styles.clone()
    }

    async fn fast_mode_state(&self) -> Option<FastModeState> {
        // Fast-mode state is runtime-tracked by the rate limiter and
        // not currently exposed. Advertise `None` until an accessor
        // lands.
        None
    }
}

/// Shared projection from a coco-rs [`AgentDefinition`] to the
/// [`SdkAgentInfo`] wire shape. Missing descriptions become `""` to
/// satisfy the required string field.
fn def_to_sdk_agent_info(def: AgentDefinition) -> SdkAgentInfo {
    SdkAgentInfo {
        name: def.name,
        description: def.description.unwrap_or_default(),
        model: def.model,
    }
}

/// Map a resolved [`AuthMethod`] to the `SdkAccountInfo` wire shape.
///
/// - **Third-party providers** (Bedrock / Vertex / Foundry): returns
///   [`SdkAccountInfo::default`] (all fields `None`), serializing to
///   `{}` on the wire to indicate no first-party account info.
/// - **First-party OAuth**: `api_provider = FirstParty`,
///   `subscription_type` from the token, `organization` is the raw
///   `org_uuid` (human-readable name requires a separate API call we
///   don't make yet). `token_source` is intentionally `None` — the
///   canonical token-source strings don't map cleanly from the
///   `AuthMethod::OAuth` variant and sending an incompatible string
///   would mislead SDK consumers that key on those values. `email` is
///   always `None` (OAuth token doesn't embed it).
/// - **First-party API key**: `api_provider = FirstParty` only.
///   `api_key_source` stays `None` until coco-rs tracks the env var /
///   helper origin (`user` / `project` / `org` / `temporary` / `oauth`).
pub fn auth_method_to_account(auth: &AuthMethod) -> SdkAccountInfo {
    match auth {
        AuthMethod::OAuth(tokens) => SdkAccountInfo {
            email: None,
            organization: tokens.org_uuid.clone(),
            subscription_type: tokens.subscription_type.clone(),
            token_source: None,
            api_key_source: None,
            api_provider: Some(SdkApiProvider::FirstParty),
        },
        AuthMethod::ApiKey { .. } => SdkAccountInfo {
            api_provider: Some(SdkApiProvider::FirstParty),
            ..Default::default()
        },
        // Third-party provider paths: return a bare default so the wire
        // shape signals no first-party account. SDK consumers that check
        // `account.apiProvider === undefined` to detect 3P auth would
        // otherwise see a populated apiProvider and treat the session as
        // "logged in".
        AuthMethod::Bedrock { .. } | AuthMethod::Vertex { .. } | AuthMethod::Foundry { .. } => {
            SdkAccountInfo::default()
        }
    }
}

/// Parse the `agents` map from a `SDKControlInitializeRequest` into
/// validated [`AgentDefinition`] entries.
///
/// The map's keys ARE the agent type names (authoritative); each value
/// is the agent's JSON shape. Per entry:
///
/// 1. Deserialize the value as `AgentDefinition` (forgiving — missing
///    optional fields default).
/// 2. Override `agent_type` with the map key (the key wins over any
///    name embedded in the value).
/// 3. Stamp `source = AgentSource::FlagSettings`.
/// 4. Run `AgentDefinitionValidator::check` — drop the entry on
///    semantic errors but keep going on the rest.
///
/// Returns `(accepted, errors)`. Caller logs `errors` at warn level
/// and proceeds with `accepted` — parse errors don't fail the
/// initialize handshake.
pub fn parse_sdk_agent_definitions(
    agents: &HashMap<String, SdkAgentDefinition>,
) -> (Vec<AgentDefinition>, Vec<String>) {
    let mut accepted = Vec::with_capacity(agents.len());
    let mut errors = Vec::new();
    for (name, sdk_def) in agents {
        // Step 1: lower the wire DTO into the internal AgentDefinition.
        let mut def = sdk_agent_definition_to_internal(sdk_def);
        // Step 2: key wins over any embedded `name`/`agent_type`.
        def.name = name.clone();
        def.agent_type = match AgentTypeId::from_str(name) {
            // AgentTypeId::from_str is `Infallible` — `Custom(name)` for
            // anything that doesn't match a built-in. Unwrap is safe.
            Ok(t) => t,
            Err(_) => AgentTypeId::Custom(name.clone()),
        };
        // Step 3: SDK-supplied agents are FlagSettings source.
        def.source = AgentSource::FlagSettings;
        // Step 4: semantic validation.
        let semantic_errors = AgentDefinitionValidator::check(&def);
        if !semantic_errors.is_empty() {
            errors.push(format!(
                "agent '{name}': validation failed: {}",
                semantic_errors
                    .iter()
                    .map(|e| format!("{e:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        }
        accepted.push(def);
    }
    (accepted, errors)
}

/// Lower an `SdkAgentDefinition` (wire DTO) into the richer internal
/// [`AgentDefinition`] used by the subagent runtime. Fields the SDK
/// doesn't expose (color, identity, required_mcp_servers, isolation,
/// pending_snapshot_update, …) fall back to defaults.
fn sdk_agent_definition_to_internal(sdk: &SdkAgentDefinition) -> AgentDefinition {
    let allowed_tools = match sdk.tools.as_ref() {
        None => ToolAllowList::Wildcard,
        Some(list) => ToolAllowList::from_frontmatter(list.clone()),
    };
    let permission_mode = sdk.permission_mode.and_then(|m| {
        // PermissionMode serializes as a JSON string (camelCase). The
        // internal AgentDefinition holds permission_mode as Option<String>
        // for legacy reasons — round-trip through serde to obtain the
        // canonical wire spelling without hard-coding a match.
        serde_json::to_value(m)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
    });
    AgentDefinition {
        description: Some(sdk.description.clone()),
        system_prompt: Some(sdk.prompt.clone()),
        allowed_tools,
        disallowed_tools: sdk.disallowed_tools.clone().unwrap_or_default(),
        model: sdk.model.clone(),
        mcp_servers: sdk.mcp_servers.clone().unwrap_or_default(),
        critical_system_reminder: sdk.critical_system_reminder_experimental.clone(),
        skills: sdk.skills.clone().unwrap_or_default(),
        initial_prompt: sdk.initial_prompt.clone(),
        max_turns: sdk.max_turns,
        background: sdk.background.unwrap_or(false),
        memory_scope: sdk.memory,
        effort: sdk.effort,
        permission_mode,
        ..AgentDefinition::default()
    }
}

#[cfg(test)]
#[path = "cli_bootstrap.test.rs"]
mod tests;
