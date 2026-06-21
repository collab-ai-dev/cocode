use std::sync::Arc;

use clap::Parser;
use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_types::ProviderModelSelection;
use tempfile::TempDir;

use super::SessionRuntime;
use super::SessionRuntimeBuildOpts;
use crate::Cli;

async fn build_runtime(home: &TempDir) -> Arc<SessionRuntime> {
    let settings = SettingsWithSource {
        merged: Settings {
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
    let model_id = crate::headless::resolve_main_model(&runtime_config).model_id;
    let cli = Cli::try_parse_from(["coco"]).expect("parse default cli");

    SessionRuntime::build(SessionRuntimeBuildOpts {
        cli: &cli,
        runtime_config: Arc::new(runtime_config),
        cwd: home.path().to_path_buf(),
        model_id,
        system_prompt: "test".to_string(),
        bypass_permissions_available: false,
        permission_mode: coco_types::PermissionMode::default(),
        model_runtimes: None,
        tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
        session_manager: Arc::new(coco_session::SessionManager::new(
            home.path().join("sessions"),
        )),
        fast_model_spec: None,
        permission_bridge: None,
        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
            coco_commands::CommandRegistry::new(),
        ))),
        skill_manager: Arc::new(coco_skills::SkillManager::new()),
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        session_id_override: None,
        is_non_interactive: false,
    })
    .await
    .expect("build SessionRuntime")
}

#[tokio::test]
async fn orchestration_ctx_factory_can_run_inside_runtime_thread() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let factory = runtime.orchestration_ctx_factory();

    let initial = factory();
    assert_eq!(initial.session_id, runtime.current_session_id().await);

    runtime
        .update_engine_config(|cfg| {
            cfg.disable_all_hooks = true;
            cfg.allow_managed_hooks_only = true;
        })
        .await;
    let updated_config = factory();
    assert!(updated_config.disable_all_hooks);
    assert!(updated_config.allow_managed_hooks_only);

    runtime.start_new_session("next-session".to_string()).await;
    let updated_session = factory();
    assert_eq!(updated_session.session_id, "next-session");
}

#[tokio::test]
async fn reload_plugin_mcp_servers_noops_without_manager_then_bumps_key_when_attached() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let cwd = home.path().to_path_buf();

    // No manager attached → no-op, reconnect key untouched.
    assert_eq!(runtime.reload_plugin_mcp_servers(&cwd).await, 0);
    assert_eq!(runtime.mcp_reconnect_key(), 0);

    // Attach a manager → reload runs the manager path and bumps the key, even
    // when no plugins contribute MCP servers (count 0, key still moves).
    let manager = Arc::new(tokio::sync::Mutex::new(
        coco_mcp::McpConnectionManager::new(home.path().to_path_buf()),
    ));
    runtime.attach_mcp_manager(manager.clone()).await;
    let count = runtime.reload_plugin_mcp_servers(&cwd).await;
    assert_eq!(count, 0, "no plugins → no plugin MCP servers");
    assert_eq!(runtime.mcp_reconnect_key(), 1, "reconnect key bumps once");
    assert!(
        manager.lock().await.registered_server_names().is_empty(),
        "no plugin servers were registered"
    );

    // A second reload bumps again (idempotent re-register, monotonic key).
    runtime.reload_plugin_mcp_servers(&cwd).await;
    assert_eq!(runtime.mcp_reconnect_key(), 2);
}

#[tokio::test]
async fn start_new_session_loads_existing_usage_snapshot() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let snapshot = coco_types::SessionUsageSnapshot {
        session_id: "resume-session".into(),
        totals: coco_types::SessionUsageTotals {
            input_tokens: 123,
            output_tokens: 45,
            request_count: 1,
            ..Default::default()
        },
        models: vec![coco_types::SessionModelUsageEntry {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-5".into(),
            input_tokens: 123,
            output_tokens: 45,
            request_count: 1,
            priced: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    runtime
        .transcript_store
        .write_usage_snapshot("resume-session", &snapshot)
        .expect("usage snapshot should write");

    runtime
        .start_new_session("resume-session".to_string())
        .await;

    assert_eq!(
        runtime.session_usage_snapshot().await.totals.input_tokens,
        123
    );
}
