//! Local AppServer host assembly shared by the TUI and headless surfaces.
//!
//! Both surfaces need the identical local-bridge wiring: build the
//! `SessionRuntimeFactory`, wrap it in an `AppServerLocalBridge` through
//! `HostInputs` + `RuntimeReplacementContext`, and attach process-owned Event
//! Hub egress (spawn + egress binding + membership watcher). This is the local
//! counterpart to [`crate::remote_host::HostBuilder`]/`PreparedHost` for the
//! SDK path.
//!
//! Owning the sequence in one place means TUI and headless differ only in the
//! session-construction policy they pass in ([`LocalHostInputs`]) and the
//! lifecycle calls (`session/start` vs `session/resume`, plus `keep_alive`)
//! they make on the returned bridge — not in the assembly itself.

use std::path::PathBuf;
use std::sync::Arc;

use coco_app_runtime::ProcessRuntime;
use coco_config::{RuntimeConfig, global_config};
use coco_session::SessionManager;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_types::{CoreEvent, ModelSpec};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::AgentHostOptions;
use crate::app_server_host::{
    AppServerLocalBridge, HostInputs, RuntimeReplacementContext, SessionTurnExecutor,
};
use crate::event_hub::{ProcessEventHub, spawn_app_server_membership_watcher};
use crate::session_bootstrap::SessionIntegrationOptions;
use crate::session_runtime::{SessionRuntimeFactory, SessionRuntimeFactoryHostConfig};

/// Whether the surface wants a plugin-directory change watcher.
///
/// Interactive TUI enables it so `.md`/manifest edits hot-reload the plugin
/// catalog mid-session; one-shot headless disables it because the process exits
/// after a single turn, so a watcher would never usefully fire. Making it an
/// explicit input keeps that divergence intentional rather than an accidental
/// omission of one surface.
pub enum LocalPluginWatch {
    Disabled,
    Enabled(mpsc::Sender<CoreEvent>),
}

/// Construction inputs shared by the TUI and headless local hosts.
///
/// The fields that genuinely differ between the two surfaces — model runtimes,
/// fast-model spec, permission bridge, interaction mode, integration policy,
/// and plugin watching — are explicit so the divergence lives in one visible
/// place instead of being duplicated across two hand-rolled call sites.
pub struct LocalHostInputs {
    pub cli: Arc<AgentHostOptions>,
    pub cwd: PathBuf,
    pub session_manager: Arc<SessionManager>,
    pub process_runtime: Arc<ProcessRuntime>,
    pub model_runtimes: Option<Arc<coco_inference::ModelRuntimeRegistry>>,
    pub fast_model_spec: Option<ModelSpec>,
    pub permission_bridge: Option<ToolPermissionBridgeRef>,
    pub builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    pub is_non_interactive: bool,
    pub integration_options: SessionIntegrationOptions,
    pub bypass_permissions_available: bool,
    pub requires_structured_output: bool,
    pub plugin_watch: LocalPluginWatch,
}

/// The assembled local host: a bridge ready for `session/start` /
/// `session/resume`, plus the process-owned Event Hub egress and plugin-watch
/// guard the surface must keep alive for the session's lifetime.
pub struct PreparedLocalHost {
    pub bridge: AppServerLocalBridge,
    pub event_hub_connector: Option<ProcessEventHub>,
    pub event_hub_membership_watcher: Option<JoinHandle<()>>,
    /// Held by the surface so the plugin watcher's notify thread + throttle
    /// task run until shutdown; dropping it stops the watcher. `None` when the
    /// surface opted out ([`LocalPluginWatch::Disabled`]).
    pub plugin_watcher_guard: Option<Arc<dyn Send + Sync>>,
}

/// Build the shared local AppServer host: factory → local bridge → Event Hub
/// egress (+ membership watcher) → optional plugin watcher. Building creates
/// zero live sessions; the caller then issues `session/start` or
/// `session/resume` on the returned bridge and `keep_alive`s the handler.
pub fn build_local_host(
    inputs: LocalHostInputs,
    runtime_config: &RuntimeConfig,
) -> PreparedLocalHost {
    let runtime_factory =
        SessionRuntimeFactory::from_host_config(SessionRuntimeFactoryHostConfig {
            cli: Arc::clone(&inputs.cli),
            cwd: inputs.cwd.clone(),
            model_runtimes: inputs.model_runtimes,
            session_manager: Arc::clone(&inputs.session_manager),
            fast_model_spec: inputs.fast_model_spec,
            permission_bridge: inputs.permission_bridge,
            process_runtime: Arc::clone(&inputs.process_runtime),
            builtin_agent_catalog: inputs.builtin_agent_catalog,
            is_non_interactive: inputs.is_non_interactive,
        });
    let bridge = AppServerLocalBridge::with_host_inputs_and_server_config(
        HostInputs {
            startup_cwd: Some(inputs.cwd.clone()),
            session_manager: Some(Arc::clone(&inputs.session_manager)),
            bypass_permissions_available: inputs.bypass_permissions_available,
            runtime_replacement: Some(RuntimeReplacementContext {
                runtime_factory,
                process_runtime: Arc::clone(&inputs.process_runtime),
                cwd: inputs.cwd.clone(),
                requires_structured_output: inputs.requires_structured_output,
                integration_options: inputs.integration_options,
            }),
            turn_runner: Some(Arc::new(SessionTurnExecutor::new(None, None))),
            ..Default::default()
        },
        &runtime_config.server,
    );
    let event_hub_connector = ProcessEventHub::spawn(runtime_config, &inputs.cwd, Vec::new());
    let event_hub_membership_watcher = event_hub_connector.as_ref().map(|connector| {
        bridge.set_hub_connector_egress(connector.egress());
        spawn_app_server_membership_watcher(Arc::clone(bridge.app_server()), connector.updater())
    });
    let plugin_watcher_guard = match inputs.plugin_watch {
        LocalPluginWatch::Enabled(event_sink) => {
            crate::plugin_watch::spawn(event_sink, &inputs.cwd, &global_config::config_home())
                .map(|guard| guard as Arc<dyn Send + Sync>)
        }
        LocalPluginWatch::Disabled => None,
    };
    PreparedLocalHost {
        bridge,
        event_hub_connector,
        event_hub_membership_watcher,
        plugin_watcher_guard,
    }
}
