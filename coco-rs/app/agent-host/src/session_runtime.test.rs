use std::sync::Arc;

use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_types::ProviderModelSelection;
use coco_types::ReasoningEffort;
use coco_types::SessionId;
use coco_types::ThinkingLevel;
use tempfile::TempDir;

use super::SessionRuntime;
use super::SessionRuntimeBootstrap;
use super::SessionRuntimeBootstrapSource;
use super::SessionRuntimeFactory;
use super::SessionRuntimeFactoryOpts;
use super::resolve_model_selection_from_runtime_config;
use super::thinking_level_for_effort_from;
use crate::AgentHostOptions;

async fn build_runtime(home: &TempDir) -> Arc<SessionRuntime> {
    build_runtime_with_main(home, "anthropic", "claude-opus-4-7").await
}

async fn build_runtime_with_main(
    home: &TempDir,
    provider: &str,
    model_id: &str,
) -> Arc<SessionRuntime> {
    try_build_runtime_with_main(home, provider, model_id, None)
        .await
        .expect("build SessionRuntime")
}

async fn try_build_runtime_with_main(
    home: &TempDir,
    provider: &str,
    model_id: &str,
    session_id_override: Option<SessionId>,
) -> anyhow::Result<Arc<SessionRuntime>> {
    let settings = SettingsWithSource {
        merged: Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(RoleSlots::new(ProviderModelSelection {
                    provider: provider.into(),
                    model_id: model_id.into(),
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
    let cli = AgentHostOptions::default();

    let factory = SessionRuntimeFactory::new(SessionRuntimeFactoryOpts {
        cli: Arc::new(cli),
        bootstrap_source: SessionRuntimeBootstrapSource::startup_snapshot(
            SessionRuntimeBootstrap {
                runtime_config: Arc::new(runtime_config),
                tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
                model_id,
                system_prompt: "test".to_string(),
                permission_mode_availability: coco_types::PermissionModeAvailability::default(),
                permission_mode: coco_types::PermissionMode::default(),
                command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
                    coco_commands::CommandRegistry::new(),
                ))),
                skill_manager: Arc::new(coco_skills::SkillManager::new()),
                project_services: Arc::new(coco_app_runtime::ProjectServices::load(
                    home.path(),
                    home.path(),
                )),
                agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
            },
        ),
        cwd: home.path().to_path_buf(),
        model_runtimes: None,
        session_manager: Arc::new(coco_session::SessionManager::new(
            home.path().join("sessions"),
        )),
        fast_model_spec: None,
        permission_bridge: None,
        process_runtime: coco_app_runtime::ProcessRuntime::global(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        is_non_interactive: false,
    });
    factory
        .build(session_id_override)
        .await
        .map(|handle| handle.runtime().clone())
}

#[tokio::test]
async fn build_uses_typed_session_id_override() {
    let home = TempDir::new().expect("home tempdir");
    let session_id = SessionId::try_new("override-session").expect("valid session id");

    let runtime = try_build_runtime_with_main(
        &home,
        "anthropic",
        "claude-opus-4-7",
        Some(session_id.clone()),
    )
    .await
    .expect("build should accept typed session id override");

    assert_eq!(runtime.current_typed_session_id().await, session_id);
}

#[tokio::test]
async fn factory_fresh_builds_create_distinct_runtime_identities() {
    let home = TempDir::new().expect("home tempdir");
    let first = try_build_runtime_with_main(&home, "anthropic", "claude-opus-4-7", None)
        .await
        .expect("build first runtime");
    let second = try_build_runtime_with_main(&home, "anthropic", "claude-opus-4-7", None)
        .await
        .expect("build second runtime");

    assert_ne!(
        first.current_typed_session_id().await,
        second.current_typed_session_id().await
    );
}

#[tokio::test]
async fn unsafe_session_id_override_is_rejected_before_runtime_build() {
    let result = SessionId::try_new("bad/session");

    assert!(result.is_err(), "unsafe session id must fail");
}

#[tokio::test]
async fn main_runtime_snapshot_uses_main_model_context_metadata() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime_with_main(&home, "deepseek-openai", "deepseek-v4-pro").await;

    let snapshot = runtime
        .model_runtimes()
        .snapshot_for_role(coco_types::ModelRole::Main)
        .expect("main runtime snapshot");
    let info = snapshot.model_info.expect("main runtime model info");

    assert_eq!(info.context_window.get(), 1_000_000);
    assert_eq!(info.max_output_tokens.get(), 12_288);
}

#[tokio::test]
async fn sdk_model_selection_resolves_bare_model_against_main_provider() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime_with_main(&home, "deepseek-openai", "deepseek-v4-pro").await;

    let bare = runtime
        .resolve_model_selection("deepseek-v4-pro")
        .expect("bare model should resolve");
    assert_eq!(bare.provider, "deepseek-openai");
    assert_eq!(bare.model_id, "deepseek-v4-pro");

    let explicit = runtime
        .resolve_model_selection("deepseek-openai/deepseek-v4-pro")
        .expect("explicit model should resolve");
    assert_eq!(explicit.provider, "deepseek-openai");
    assert_eq!(explicit.model_id, "deepseek-v4-pro");
}

#[test]
fn sdk_model_selection_accepts_configured_moa_preset() {
    let home = TempDir::new().expect("home tempdir");
    let mut presets = std::collections::BTreeMap::new();
    presets.insert(
        "default".to_string(),
        coco_config::MoaPresetSettings {
            aggregator: Some(ProviderModelSelection {
                provider: "anthropic".to_string(),
                model_id: "claude-sonnet-4-6".to_string(),
            }),
            reference_models: vec![ProviderModelSelection {
                provider: "openai".to_string(),
                model_id: "gpt-5-4".to_string(),
            }],
            ..Default::default()
        },
    );
    let settings = SettingsWithSource {
        merged: Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(RoleSlots::new(ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-sonnet-4-6".into(),
                })),
                ..Default::default()
            },
            moa: coco_config::MoaSettings {
                default_preset: Some("default".to_string()),
                presets,
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

    let selection = resolve_model_selection_from_runtime_config(&runtime_config, "moa/default")
        .expect("configured MoA preset should resolve");

    assert_eq!(selection.provider, "moa");
    assert_eq!(selection.model_id, "default");
    assert!(resolve_model_selection_from_runtime_config(&runtime_config, "moa/missing").is_none());
}

#[test]
fn thinking_level_for_effort_uses_current_model_metadata() {
    let model = coco_config::ModelInfo {
        supported_thinking_levels: Some(vec![ThinkingLevel::with_budget(
            ReasoningEffort::High,
            32_000,
        )]),
        ..Default::default()
    };

    let level = thinking_level_for_effort_from(Some(&model), ReasoningEffort::High);

    assert_eq!(level.effort, ReasoningEffort::High);
    assert_eq!(level.budget_tokens, Some(32_000));
}

#[tokio::test]
async fn orchestration_ctx_factory_can_run_inside_runtime_thread() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let factory = runtime.orchestration_ctx_factory();

    let initial = factory();
    assert_eq!(initial.session_id, runtime.current_typed_session_id().await);

    runtime
        .update_engine_config(|cfg| {
            cfg.disable_all_hooks = true;
            cfg.allow_managed_hooks_only = true;
        })
        .await;
    let updated_config = factory();
    assert!(updated_config.disable_all_hooks);
    assert!(updated_config.allow_managed_hooks_only);
}

#[tokio::test]
async fn reload_plugin_mcp_servers_noops_without_manager_then_bumps_key_when_attached() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;

    // No manager attached → no-op, reconnect key untouched.
    assert_eq!(runtime.reload_plugin_mcp_servers().await, 0);
    assert_eq!(runtime.mcp_reconnect_key(), 0);

    // Attach a manager → reload runs the manager path and bumps the key, even
    // when no plugins contribute MCP servers (count 0, key still moves).
    let manager = Arc::new(tokio::sync::Mutex::new(
        coco_mcp::McpConnectionManager::new(home.path().to_path_buf()),
    ));
    runtime.attach_mcp_manager(manager.clone()).await;
    let count = runtime.reload_plugin_mcp_servers().await;
    assert_eq!(count, 0, "no plugins → no plugin MCP servers");
    assert_eq!(runtime.mcp_reconnect_key(), 1, "reconnect key bumps once");
    assert!(
        manager.lock().await.registered_server_names().is_empty(),
        "no plugin servers were registered"
    );

    // A second reload bumps again (idempotent re-register, monotonic key).
    runtime.reload_plugin_mcp_servers().await;
    assert_eq!(runtime.mcp_reconnect_key(), 2);
}
