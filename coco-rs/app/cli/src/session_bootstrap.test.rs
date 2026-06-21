//! Tests for [`session_bootstrap`].
//!
//! `install_session_late_binds` is the parity-enforcement helper —
//! these tests pin its contract: every late-bind slot is populated
//! after a successful call, and `mcp_handle` is honored as `Some` /
//! ignored as `None`.

use std::sync::Arc;

use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_types::ProviderModelSelection;
use tempfile::TempDir;

use crate::Cli;
use crate::session_bootstrap::install_session_late_binds;
use crate::session_runtime::SessionRuntime;
use crate::session_runtime::SessionRuntimeBuildOpts;

/// Build a fresh `SessionRuntime` against a tempdir-backed runtime
/// config so the test runs hermetically (no `~/.coco` reads/writes).
async fn build_runtime(home: &TempDir) -> Arc<SessionRuntime> {
    use clap::Parser;

    let settings = SettingsWithSource {
        merged: Settings {
            // Multi-LLM SDK: Main is mandatory, no implicit default.
            // Tests pin a builtin model so SessionRuntime can build.
            models: coco_config::ModelSelectionSettings {
                main: Some(RoleSlots::new(ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-opus-4-7".into(),
                })),
                ..Default::default()
            },
            ..Default::default()
        },
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };
    let runtime_config = coco_config::build_runtime_config_with(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::empty_in(home.path()),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config");

    // Resolve the model identity the runtime config will bind.
    let model_id = crate::headless::resolve_main_model(&runtime_config).model_id;

    let registry = coco_tool_runtime::ToolRegistry::new();
    let tools = Arc::new(registry);

    let cwd = home.path().to_path_buf();
    let cli = Cli::try_parse_from(["coco"]).expect("parse default cli");
    let session_manager = Arc::new(coco_session::SessionManager::new(home.path().to_path_buf()));

    let command_registry = Arc::new(tokio::sync::RwLock::new(Arc::new(
        coco_commands::CommandRegistry::new(),
    )));
    let skill_manager = Arc::new(coco_skills::SkillManager::new());

    SessionRuntime::build(SessionRuntimeBuildOpts {
        cli: &cli,
        runtime_config: Arc::new(runtime_config),
        cwd,
        model_id,
        system_prompt: "test".to_string(),
        bypass_permissions_available: false,
        permission_mode: coco_types::PermissionMode::default(),
        model_runtimes: None,
        tools,
        session_manager,
        fast_model_spec: None,
        permission_bridge: None,
        command_registry,
        skill_manager,
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        session_id_override: None,
        is_non_interactive: false,
    })
    .await
    .expect("build SessionRuntime")
}

#[tokio::test]
async fn install_session_late_binds_populates_every_slot_without_mcp() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let cwd = home.path().to_path_buf();

    install_session_late_binds(runtime.clone(), &cwd, None, None, None)
        .await
        .expect("install_session_late_binds");

    assert!(
        runtime.current_task_runtime().await.is_some(),
        "task_runtime slot must be populated"
    );
    assert!(
        runtime.current_agent_transcript_store().await.is_some(),
        "agent_transcript_store slot must be populated"
    );
    assert!(
        runtime.current_fork_dispatcher().await.is_some(),
        "fork_dispatcher slot must be populated"
    );
    assert!(
        runtime.current_mcp_handle().await.is_none(),
        "mcp_handle slot must stay None when caller passes None"
    );
    assert!(
        runtime.current_lsp_handle().await.is_none(),
        "lsp_handle slot must stay None when caller passes None"
    );
}

#[tokio::test]
async fn install_session_late_binds_attaches_mcp_when_some() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let cwd = home.path().to_path_buf();

    let mcp_handle: coco_tool_runtime::McpHandleRef = Arc::new(coco_tool_runtime::NoOpMcpHandle);

    install_session_late_binds(runtime.clone(), &cwd, Some(mcp_handle), None, None)
        .await
        .expect("install_session_late_binds");

    assert!(
        runtime.current_mcp_handle().await.is_some(),
        "mcp_handle slot must be Some when caller passes Some"
    );
}

#[tokio::test]
async fn bootstrap_session_mcp_attaches_handle_and_manager_with_no_servers() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let cwd = home.path().to_path_buf();

    // Hermetic tempdir → no config-file or plugin MCP servers. Bootstrap must
    // still attach the manager + an `McpManagerAdapter` handle (the background
    // connect pass simply has nothing to connect).
    crate::session_bootstrap::bootstrap_session_mcp(
        &runtime, &cwd, None, /*await_connect*/ true,
    )
    .await;

    assert!(
        runtime.current_mcp_handle().await.is_some(),
        "bootstrap must attach an MCP handle even with no servers"
    );
    // A manager is now attached, so `reload_plugin_mcp_servers` runs the manager
    // path (returns 0 servers but bumps the reconnect key from 0 → 1). Without
    // `attach_mcp_manager` it would have no-op'd at key 0.
    assert_eq!(runtime.reload_plugin_mcp_servers(&cwd).await, 0);
    assert_eq!(runtime.mcp_reconnect_key(), 1);
}

#[tokio::test]
async fn install_session_late_binds_attaches_lsp_when_some() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let cwd = home.path().to_path_buf();

    let lsp_handle: coco_tool_runtime::LspHandleRef = Arc::new(coco_tool_runtime::NoOpLspHandle);

    install_session_late_binds(runtime.clone(), &cwd, None, Some(lsp_handle), None)
        .await
        .expect("install_session_late_binds");

    assert!(
        runtime.current_lsp_handle().await.is_some(),
        "lsp_handle slot must be Some when caller passes Some"
    );
}

// ---------------------------------------------------------------------------
// reconcile_mcp_server_registration — per-server auth surfacing + strand guard
// ---------------------------------------------------------------------------

fn http_server(name: &str) -> coco_mcp::types::ScopedMcpServerConfig {
    coco_mcp::types::ScopedMcpServerConfig {
        name: name.into(),
        config: coco_mcp::types::McpServerConfig::Http(coco_mcp::types::McpHttpConfig {
            url: "https://mcp.example.test/api".into(),
            headers: Default::default(),
            headers_helper: None,
            oauth: None,
        }),
        scope: coco_mcp::types::ConfigScope::User,
        plugin_source: None,
    }
}

fn auth_tool_id(server: &str) -> coco_types::ToolId {
    coco_types::ToolId::Mcp {
        server: server.into(),
        tool: "authenticate".into(),
    }
}

#[tokio::test]
async fn reconcile_surfaces_auth_tool_for_needs_auth_server() {
    let mut manager = coco_mcp::McpConnectionManager::new(std::env::temp_dir());
    manager.register_server(http_server("remote"));
    manager.mark_needs_auth("remote").await;

    let registry = coco_tool_runtime::ToolRegistry::new();
    crate::session_bootstrap::reconcile_mcp_server_registration(&manager, &registry, "remote")
        .await;

    assert!(
        registry.get(&auth_tool_id("remote")).is_some(),
        "a NeedsAuth server must surface its per-server authenticate tool"
    );
}

#[tokio::test]
async fn reconcile_does_not_strand_auth_tool_on_non_connected_state() {
    // Strand-hole guard: reconcile on a non-connected state (here: unknown /
    // Pending) must NOT deregister an already-surfaced auth tool.
    let manager = coco_mcp::McpConnectionManager::new(std::env::temp_dir());
    let registry = coco_tool_runtime::ToolRegistry::new();
    coco_tools::register_mcp_auth_tool(&registry, "remote", "http", Some("https://x"));

    crate::session_bootstrap::reconcile_mcp_server_registration(&manager, &registry, "remote")
        .await;

    assert!(
        registry.get(&auth_tool_id("remote")).is_some(),
        "reconcile on a non-connected state must not strand the auth tool"
    );
}
