use coco_types::InitializeSlashCommand;
use coco_types::{
    InitializeAccountInfo, InitializeAgentInfo, InitializeModelInfo, InitializeResult,
};
use tracing::info;

use crate::app_server_host::request_handlers::{
    APP_SERVER_PROTOCOL_VERSION, DEFAULT_APP_SERVER_FAST_MODEL, DEFAULT_APP_SERVER_MODEL,
    HandlerContext, HandlerResult,
    initialize_metadata::{runtime_fast_mode_state, runtime_initialize_metadata},
};
use crate::session_runtime::SessionAccountProvider;
use crate::session_runtime::SessionInitializeAccount;
use crate::session_runtime::SessionInitializeAgent;
use crate::session_runtime::SessionInitializeCommand;

/// `initialize` — capability negotiation. Returns an `InitializeResult`.
///
/// Data sourcing:
/// - `models`: static list of the two Anthropic models coco-rs ships with
///  (promoted from a fixed table; model discovery is a separate follow-up).
/// - `commands`, `agents`, `output_style`, `available_output_styles`:
///   populated from the live [`SessionHandle`] when installed, so initialize
///   reflects the active runtime after replacements. Before a runtime exists,
///   these fall back to the optional
///   [`crate::app_server_host::InitializeBootstrap`] snapshot provided through
///   [`crate::app_server_host::HostInputs`].
/// - `fast_mode_state`: populated from the live [`SessionHandle`] when
///   installed, falling back to the optional bootstrap provider before runtime
///   construction.
/// - `account`: populated from the optional bootstrap provider until auth
///   sources grow runtime-owned accessors.
/// - Internal `_cocoRs*` extension fields carry the coco-rs binary and
///   protocol version for debugging.
pub(crate) async fn handle_initialize(
    _params: coco_types::InitializeParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    info!("AppServerHost: initialize");
    let params = ctx.connection_profile.initialize();

    // When the remote client pushes `initialize.agents`, parse the JSON map
    // into `AgentDefinition` entries (tagged `AgentSource::FlagSettings`)
    // and stash them on host bootstrap state so:
    //   - the `agents()` listing below merges them into the response;
    //   - `session/start` drains the stash into the new session's
    //     `AgentDefinitionStore`.
    //
    // Parse failures don't fail the initialize handshake — log and
    // continue with the accepted subset.
    let accepted_client_agents = if let Some(agents_map) = params.agents.as_ref() {
        let (accepted, errors) =
            crate::app_server_host::initialize_agents::parse_client_agent_definitions(agents_map);
        if !errors.is_empty() {
            for err in &errors {
                tracing::warn!(target: "coco::app_server_host::initialize", "client agent parse error: {err}");
            }
        }
        accepted
    } else {
        Vec::new()
    };

    // Pull the bootstrap provider out of state, drop the read guard, then
    // call its async accessors. Holding the guard across awaits would
    // block any concurrent mutation (e.g. a hot-swap via builder).
    let bootstrap = ctx.state.initialize_bootstrap_snapshot().await;
    let runtime = ctx.resolve_runtime().await;

    let (commands, mut agents, output_style, available_output_styles) =
        if let Some(runtime) = runtime.as_ref() {
            let metadata = runtime_initialize_metadata(runtime).await;
            (
                metadata.commands,
                metadata.agents,
                metadata.output_style,
                metadata.available_output_styles,
            )
        } else if let Some(b) = bootstrap.as_ref() {
            (
                b.commands().await,
                b.agents().await,
                b.output_style().await,
                b.available_output_styles().await,
            )
        } else {
            (
                Vec::new(),
                Vec::new(),
                "default".into(),
                vec!["default".into()],
            )
        };

    let account = if let Some(b) = bootstrap.as_ref() {
        b.account().await
    } else {
        SessionInitializeAccount::default()
    };
    let fast_mode_state = if let Some(runtime) = runtime.as_ref() {
        Some(runtime_fast_mode_state(runtime).await)
    } else if let Some(b) = bootstrap.as_ref() {
        b.fast_mode_state().await
    } else {
        None
    };

    // Merge client-supplied agents into the response listing so the client
    // immediately sees what it pushed. Stashed entries always win —
    // they're the freshest user intent.
    {
        let stash = accepted_client_agents;
        if !stash.is_empty() {
            let stash_names: std::collections::HashSet<String> =
                stash.iter().map(|d| d.agent_type.to_string()).collect();
            agents.retain(|a| !stash_names.contains(&a.name));
            agents.extend(stash.iter().cloned().map(|d| SessionInitializeAgent {
                name: d.name,
                description: d.description.unwrap_or_default(),
                model: d.model,
            }));
            agents.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    let result = InitializeResult {
        commands: commands.into_iter().map(command_to_initialize).collect(),
        agents: agents.into_iter().map(agent_to_initialize).collect(),
        output_style,
        available_output_styles,
        models: vec![
            InitializeModelInfo {
                value: DEFAULT_APP_SERVER_MODEL.into(),
                display_name: "Claude Opus 4.6".into(),
                description: "Anthropic's most capable model for deep reasoning tasks.".into(),
                supports_effort: Some(true),
                supported_effort_levels: Vec::new(),
                supports_adaptive_thinking: Some(true),
                supports_fast_mode: Some(true),
                supports_auto_mode: Some(true),
            },
            InitializeModelInfo {
                value: DEFAULT_APP_SERVER_FAST_MODEL.into(),
                display_name: "Claude Sonnet 4.6".into(),
                description: "Fast, cost-efficient model for everyday coding tasks.".into(),
                supports_effort: Some(true),
                supported_effort_levels: Vec::new(),
                supports_adaptive_thinking: Some(true),
                supports_fast_mode: Some(true),
                supports_auto_mode: Some(true),
            },
        ],
        account: account_to_initialize(account),
        pid: Some(std::process::id()),
        fast_mode_state,
        protocol_version: APP_SERVER_PROTOCOL_VERSION.into(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    HandlerResult::ok(result)
}

fn command_to_initialize(command: SessionInitializeCommand) -> InitializeSlashCommand {
    InitializeSlashCommand {
        name: command.name,
        description: command.description,
        argument_hint: command.argument_hint,
    }
}

fn agent_to_initialize(agent: SessionInitializeAgent) -> InitializeAgentInfo {
    InitializeAgentInfo {
        name: agent.name,
        description: agent.description,
        model: agent.model,
    }
}

fn account_to_initialize(account: SessionInitializeAccount) -> InitializeAccountInfo {
    InitializeAccountInfo {
        email: account.email,
        organization: account.organization,
        subscription_type: account.subscription_type,
        token_source: account.token_source,
        api_key_source: account.api_key_source,
        api_provider: account.api_provider.map(account_provider_to_initialize),
    }
}

fn account_provider_to_initialize(
    provider: SessionAccountProvider,
) -> coco_types::InitializeApiProvider {
    match provider {
        SessionAccountProvider::FirstParty => coco_types::InitializeApiProvider::FirstParty,
    }
}
