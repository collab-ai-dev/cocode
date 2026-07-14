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

use super::ActiveTurnHandles;
use super::SessionCloseDrainError;
use super::SessionHandle;
use super::SessionRuntimeBootstrap;
use super::SessionRuntimeBootstrapSource;
use super::SessionRuntimeFactory;
use super::SessionRuntimeFactoryOpts;
use super::resolve_model_selection_from_runtime_config;
use super::thinking_level_for_effort_from;
use crate::AgentHostOptions;

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

async fn build_runtime(home: &TempDir) -> SessionHandle {
    build_runtime_with_main(home, "anthropic", "claude-opus-4-7").await
}

async fn build_runtime_with_main(home: &TempDir, provider: &str, model_id: &str) -> SessionHandle {
    try_build_runtime_with_main(home, provider, model_id, None)
        .await
        .expect("build SessionRuntime")
}

async fn try_build_runtime_with_main(
    home: &TempDir,
    provider: &str,
    model_id: &str,
    session_id_override: Option<SessionId>,
) -> anyhow::Result<SessionHandle> {
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
        bootstrap_source: SessionRuntimeBootstrapSource::from_prebuilt_bootstrap(
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
    factory.build(session_id_override, Default::default()).await
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
async fn close_times_out_and_aborts_forwarder_task() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let session_id = runtime.session_id().clone();
    let (forwarder_drop_tx, forwarder_drop_rx) = tokio::sync::oneshot::channel();
    runtime
        .start_active_turn(|_, cancel_token| ActiveTurnHandles {
            cancel_token,
            turn_task: tokio::spawn(async {}),
            forwarder_task: tokio::spawn(async move {
                let _drop_signal = DropSignal(Some(forwarder_drop_tx));
                std::future::pending::<()>().await;
            }),
        })
        .expect("start synthetic active turn");

    let error = runtime
        .close_if_current_session(
            &session_id,
            coco_hooks::orchestration::ExitReason::Other,
            std::time::Duration::from_millis(20),
        )
        .await
        .expect_err("forwarder timeout should fail close");

    assert!(matches!(
        error,
        SessionCloseDrainError::ForwarderTaskTimeout { .. }
    ));
    forwarder_drop_rx
        .await
        .expect("timed-out forwarder task aborted");
    assert!(!runtime.has_active_turn());
}

#[tokio::test]
async fn finishing_active_turn_still_blocks_new_turn_until_cleared() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    runtime
        .start_active_turn(|_, cancel_token| ActiveTurnHandles {
            cancel_token,
            turn_task: tokio::spawn(async {}),
            forwarder_task: tokio::spawn(async {}),
        })
        .expect("start synthetic active turn");

    assert!(!runtime.complete_finishing_active_turn());
    assert!(runtime.has_active_turn());
    assert!(runtime.mark_active_turn_finishing());
    assert!(runtime.has_active_turn());
    assert!(
        runtime
            .start_active_turn(|_, cancel_token| ActiveTurnHandles {
                cancel_token,
                turn_task: tokio::spawn(async {}),
                forwarder_task: tokio::spawn(async {}),
            })
            .is_err(),
        "Finishing must still reserve the turn slot"
    );

    assert!(runtime.complete_finishing_active_turn());
    assert!(!runtime.has_active_turn());
}

#[tokio::test]
async fn close_drain_waits_finishing_turn_without_new_cancel() {
    let home = TempDir::new().expect("home tempdir");
    let runtime = build_runtime(&home).await;
    let session_id = runtime.session_id().clone();
    let mut cancel_snapshot = None;
    runtime
        .start_active_turn(|_, cancel_token| {
            cancel_snapshot = Some(cancel_token.clone());
            ActiveTurnHandles {
                cancel_token,
                turn_task: tokio::spawn(async {}),
                forwarder_task: tokio::spawn(async {}),
            }
        })
        .expect("start synthetic active turn");
    assert!(runtime.mark_active_turn_finishing());

    runtime
        .close_if_current_session(
            &session_id,
            coco_hooks::orchestration::ExitReason::Other,
            std::time::Duration::from_secs(1),
        )
        .await
        .expect("finishing turn drains");

    assert!(
        !cancel_snapshot
            .expect("cancel token captured")
            .is_cancelled(),
        "close during Finishing should wait for terminal delivery instead of issuing a new cancel"
    );
    assert!(!runtime.has_active_turn());
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
async fn model_selection_resolves_bare_model_against_main_provider() {
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
fn model_selection_accepts_configured_moa_preset() {
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

#[tokio::test]
async fn model_role_selection_keeps_moa_display_binding_for_main() {
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
    let cli = AgentHostOptions::default();
    let factory = SessionRuntimeFactory::new(SessionRuntimeFactoryOpts {
        cli: Arc::new(cli),
        bootstrap_source: SessionRuntimeBootstrapSource::from_prebuilt_bootstrap(
            SessionRuntimeBootstrap {
                model_id: crate::headless::resolve_main_model(&runtime_config).model_id,
                runtime_config: Arc::new(runtime_config),
                tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
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
    let runtime = factory
        .build(None, Default::default())
        .await
        .expect("build SessionRuntime");

    let change = runtime
        .apply_model_role_selection(super::SessionModelRoleSelection {
            role: coco_types::ModelRole::Main,
            provider: coco_config::MOA_PROVIDER.to_string(),
            model_id: "default".to_string(),
            effort: Some(ReasoningEffort::High),
        })
        .await
        .expect("apply model role selection");

    assert_eq!(change.display_provider, "moa");
    assert_eq!(change.display_model_id, "default");
    assert_eq!(change.effort, Some(ReasoningEffort::High));
    assert_eq!(runtime.current_engine_config().await.model_id, "default");
    let snapshot = runtime
        .model_role_change_snapshot(coco_types::ModelRole::Main, Some(ReasoningEffort::Low))
        .await
        .expect("main role change snapshot");
    assert_eq!(snapshot.display_provider, "moa");
    assert_eq!(snapshot.display_model_id, "default");
    assert_eq!(snapshot.effort, Some(ReasoningEffort::Low));
    let resolved = runtime
        .resolve_role(coco_types::ModelRole::Main)
        .await
        .expect("resolved main role");
    assert_eq!(resolved.spec.provider, "anthropic");
    assert_eq!(resolved.spec.model_id, "claude-sonnet-4-6");
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

#[test]
fn refresh_live_permissions_preserves_plan_latches_when_mode_unchanged() {
    let mut app_state = coco_types::ToolAppState {
        permissions: coco_types::LiveToolPermissionState {
            mode: Some(coco_types::PermissionMode::Plan),
            pre_plan_mode: Some(coco_types::PermissionMode::Auto),
            stripped_dangerous_rules: Some(coco_types::PermissionRulesBySource::default()),
            ..Default::default()
        },
        plan_mode_entry_ms: Some(42),
        ..Default::default()
    };

    super::permissions::refresh_live_permissions_for_turn(
        &mut app_state,
        super::SessionTurnPermissionRefresh {
            fallback_previous_mode: coco_types::PermissionMode::Default,
            permission_mode: coco_types::PermissionMode::Plan,
            allow_rules: coco_types::PermissionRulesBySource::default(),
            deny_rules: coco_types::PermissionRulesBySource::default(),
            ask_rules: coco_types::PermissionRulesBySource::default(),
            permission_rule_source_roots: std::collections::HashMap::new(),
            plan_auto_options: coco_permissions::PlanModeAutoOptions::default(),
        },
    );

    assert_eq!(
        app_state.permissions.pre_plan_mode,
        Some(coco_types::PermissionMode::Auto)
    );
    assert!(app_state.permissions.stripped_dangerous_rules.is_some());
    assert_eq!(app_state.plan_mode_entry_ms, Some(42));
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
