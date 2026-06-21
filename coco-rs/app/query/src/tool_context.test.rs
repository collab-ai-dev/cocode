use std::sync::Arc;

use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::TurnAbortController;
use coco_tool_runtime::mcp_handle::McpResourceContent;
use coco_tool_runtime::mcp_handle::McpResourceInfo;
use coco_tool_runtime::mcp_handle::McpToolCallResult;
use coco_types::PermissionMode;
use coco_types::ThinkingLevel;
use coco_types::ToolAbortReasonPayload;
use coco_types::ToolAppState;
use coco_types::TurnAbortReason;
use pretty_assertions::assert_eq;
use tokio::sync::RwLock;

use super::*;
use crate::config::QueryEngineConfig;

fn test_config() -> QueryEngineConfig {
    QueryEngineConfig {
        model_id: "claude-test".into(),
        permission_mode: PermissionMode::Default,
        context_window: 200_000,
        max_output_tokens: 8_192,
        session_id: "session-abc".into(),
        ..Default::default()
    }
}

fn factory(config: QueryEngineConfig) -> ToolContextFactory {
    factory_with_live_rules(config, Arc::new(RwLock::new(Vec::new())))
}

/// Build a factory whose live `ToolAppState.permissions` base is pre-seeded
/// with the given rules — the real-runtime shape, since the live base (not the
/// config) is the single permission source the factory reads each batch.
fn factory_with_base_rules(
    config: QueryEngineConfig,
    permissions: coco_types::LiveToolPermissionState,
) -> ToolContextFactory {
    ToolContextFactory {
        app_state: Some(Arc::new(RwLock::new(ToolAppState {
            permissions,
            ..Default::default()
        }))),
        ..factory(config)
    }
}

fn factory_with_live_rules(
    config: QueryEngineConfig,
    live_command_rules: Arc<RwLock<Vec<coco_types::PermissionRule>>>,
) -> ToolContextFactory {
    ToolContextFactory {
        config,
        tools: Arc::new(ToolRegistry::new()),
        turn_abort: TurnAbortSignal::from_token(tokio_util::sync::CancellationToken::new()),
        mailbox: None,
        pending_messages: None,
        task_list: None,
        team_task_list_router: None,
        todo_list: None,
        task_handle: None,
        permission_bridge: None,
        app_state: None,
        file_read_state: None,
        file_history: None,
        config_home: None,
        tool_result_session_dir: None,
        transcript_path: None,
        hook_handle: None,
        agent_handle: None,
        skill_handle: None,
        lsp_handle: None,
        mcp_handle: None,
        schedule_store: None,
        agent_catalog: None,
        parent_runtime_snapshot: None,
        live_command_rules,
    }
}

#[derive(Debug)]
struct PendingMcpHandle;

#[async_trait::async_trait]
impl coco_tool_runtime::McpHandle for PendingMcpHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(Vec::new())
    }

    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        Ok(McpToolCallResult {
            content: Vec::new(),
            is_error: false,
        })
    }

    async fn authenticate(&self, _: &str) -> Result<String, coco_error::BoxedError> {
        Ok(String::new())
    }

    async fn connected_servers(&self) -> Vec<String> {
        Vec::new()
    }

    async fn pending_server_names(&self) -> Vec<String> {
        vec!["github".to_string()]
    }
}

#[tokio::test]
async fn test_factory_preserves_structured_turn_abort_reason() {
    let turn_abort = TurnAbortController::new();
    turn_abort.abort(TurnAbortReason::SubmitInterrupt);
    let ctx = ToolContextFactory {
        turn_abort: turn_abort.signal(),
        ..factory(test_config())
    }
    .build(Default::default())
    .await;

    assert_eq!(
        ctx.abort.reason(),
        Some(ToolAbortReasonPayload::Turn {
            reason: TurnAbortReason::SubmitInterrupt,
        })
    );
}

#[tokio::test]
async fn local_denial_tracking_isolated_per_subagent_shared_for_main_session() {
    // TS `createSubagentContext` parity: every subagent gets its own
    // `DenialTracker` so one child's denial streak can't trip the shared
    // auto-mode circuit breaker for the parent + siblings. The main session
    // (`agent_id == None`) keeps `None` and uses the engine-level tracker.
    let main_ctx = factory(test_config()).build(Default::default()).await;
    assert!(
        main_ctx.local_denial_tracking.is_none(),
        "main session uses the shared engine-level tracker",
    );

    let subagent_config = QueryEngineConfig {
        agent_id: Some("agent-1".into()),
        ..test_config()
    };
    let subagent_ctx = factory(subagent_config).build(Default::default()).await;
    assert!(
        subagent_ctx.local_denial_tracking.is_some(),
        "subagent gets an isolated fresh DenialTracker",
    );
}

#[tokio::test]
async fn test_factory_main_loop_model_defaults_to_config_model_id() {
    // When no current_model_id override is supplied, main_loop_model
    // falls back to the static config.model_id. This path is used by
    // tests and legacy single-client constructions without a ModelRuntime.
    let config = test_config();
    let ctx = factory(config).build(Default::default()).await;
    assert_eq!(ctx.main_loop_model, "claude-test");
}

#[tokio::test]
async fn test_factory_honors_current_model_id_override() {
    // After a fallback switch, the engine passes the active-slot model id
    // via ToolContextOverrides so tools and subagents see post-fallback
    // state instead of the static config value.
    let config = test_config();
    let ctx = factory(config)
        .build(ToolContextOverrides {
            current_model_id: Some("fallback-model".into()),
            ..Default::default()
        })
        .await;
    assert_eq!(ctx.main_loop_model, "fallback-model");
}

#[tokio::test]
async fn test_factory_threads_tool_reference_capability() {
    // The engine derives `current_model_supports_tool_reference` from
    // the active client's `ModelInfo` and passes it through overrides.
    // The factory must surface it on the built `ToolUseContext` so
    // `ToolSearchTool::execute` can branch into the cache-friendly
    // path on capable models.
    let config = test_config();
    let ctx_capable = factory(config.clone())
        .build(ToolContextOverrides {
            current_model_supports_tool_reference: true,
            ..Default::default()
        })
        .await;
    assert!(ctx_capable.model_supports_tool_reference);

    let ctx_incapable = factory(config)
        .build(ToolContextOverrides {
            current_model_supports_tool_reference: false,
            ..Default::default()
        })
        .await;
    assert!(!ctx_incapable.model_supports_tool_reference);
}

#[tokio::test]
async fn test_factory_threads_client_side_tool_search_capability() {
    // Same plumbing as `tool_reference` — the client-side capability
    // is the universal cousin (no Anthropic beta dependency). When
    // both capabilities are absent, `ctx.tool_search_supported()` is
    // false and `ToolSearch` hides from the model.
    let config = test_config();

    let ctx_neither = factory(config.clone()).build(Default::default()).await;
    assert!(!ctx_neither.model_supports_client_side_tool_search);
    assert!(
        !ctx_neither.tool_search_supported(),
        "no capability → tool_search inactive even if feature on"
    );

    let ctx_client_only = factory(config.clone())
        .build(ToolContextOverrides {
            current_model_supports_client_side_tool_search: true,
            ..Default::default()
        })
        .await;
    assert!(ctx_client_only.model_supports_client_side_tool_search);
    assert!(!ctx_client_only.model_supports_tool_reference);
    assert!(
        ctx_client_only.tool_search_supported(),
        "client-side cap alone is supported when feature on"
    );
    assert!(
        !ctx_client_only.tool_search_active(),
        "no deferred tools and no pending MCP means ToolSearch remains hidden"
    );

    let ctx_server_only = factory(config)
        .build(ToolContextOverrides {
            current_model_supports_tool_reference: true,
            ..Default::default()
        })
        .await;
    assert!(
        ctx_server_only.tool_search_supported(),
        "server-side cap alone is supported when feature on"
    );
    assert!(
        !ctx_server_only.tool_search_active(),
        "no deferred tools and no pending MCP means ToolSearch remains hidden"
    );
}

#[tokio::test]
async fn test_factory_activates_tool_search_for_deferred_tools() {
    let tools = ToolRegistry::new();
    tools.register(Arc::new(coco_tools::TaskOutputTool));
    let ctx = ToolContextFactory {
        tools: Arc::new(tools),
        ..factory(test_config())
    }
    .build(ToolContextOverrides {
        current_model_supports_client_side_tool_search: true,
        ..Default::default()
    })
    .await;

    assert!(ctx.tool_search_active());
}

#[tokio::test]
async fn test_factory_activates_tool_search_for_pending_mcp() {
    let ctx = ToolContextFactory {
        mcp_handle: Some(Arc::new(PendingMcpHandle)),
        ..factory(test_config())
    }
    .build(ToolContextOverrides {
        current_model_supports_client_side_tool_search: true,
        ..Default::default()
    })
    .await;

    assert!(ctx.tool_search_active());
}

#[tokio::test]
async fn test_factory_honors_is_non_interactive() {
    let mut config = test_config();
    config.is_non_interactive = true;
    let ctx = factory(config).build(Default::default()).await;
    assert!(ctx.is_non_interactive);
}

#[tokio::test]
async fn test_factory_honors_avoid_permission_prompts() {
    // `avoid_permission_prompts` is independent of `is_non_interactive`:
    // a session can be non-interactive yet still route `Ask` to a consumer.
    let mut config = test_config();
    config.avoid_permission_prompts = true;
    let ctx = factory(config).build(Default::default()).await;
    assert!(ctx.avoid_permission_prompts);
    assert!(!ctx.is_non_interactive);
}

#[tokio::test]
async fn test_factory_honors_max_budget_usd() {
    let mut config = test_config();
    config.max_budget_usd = Some(12.5);
    let ctx = factory(config).build(Default::default()).await;
    assert_eq!(ctx.max_budget_usd, Some(12.5));
}

#[tokio::test]
async fn test_factory_maps_system_prompt_to_custom() {
    let mut config = test_config();
    config.system_prompt = Some("custom prompt body".into());
    let ctx = factory(config).build(Default::default()).await;
    assert_eq!(
        ctx.custom_system_prompt.as_deref(),
        Some("custom prompt body")
    );
}

#[tokio::test]
async fn test_factory_honors_append_system_prompt() {
    let mut config = test_config();
    config.append_system_prompt = Some("extra rules".into());
    let ctx = factory(config).build(Default::default()).await;
    assert_eq!(ctx.append_system_prompt.as_deref(), Some("extra rules"));
}

#[tokio::test]
async fn test_factory_honors_thinking_level() {
    let mut config = test_config();
    config.thinking_level = Some(ThinkingLevel::medium());
    let ctx = factory(config).build(Default::default()).await;
    let level = ctx.thinking_level.expect("thinking level must propagate");
    assert_eq!(level.effort, ThinkingLevel::medium().effort);
}

#[tokio::test]
async fn test_factory_uses_live_permission_mode_from_app_state() {
    let mut config = test_config();
    config.permission_mode = PermissionMode::Default;
    let state = Arc::new(RwLock::new(ToolAppState::default()));
    state.write().await.permissions.mode = Some(PermissionMode::Plan);
    let f = ToolContextFactory {
        app_state: Some(state),
        ..factory(config)
    };
    let ctx = f.build(Default::default()).await;
    assert_eq!(ctx.permission_context.mode, PermissionMode::Plan);
}

#[tokio::test]
async fn test_factory_falls_back_to_config_permission_mode_without_app_state() {
    let mut config = test_config();
    config.permission_mode = PermissionMode::AcceptEdits;
    let ctx = factory(config).build(Default::default()).await;
    assert_eq!(ctx.permission_context.mode, PermissionMode::AcceptEdits);
}

#[tokio::test]
async fn test_factory_passes_user_message_id_override() {
    let ctx = factory(test_config())
        .build(ToolContextOverrides {
            user_message_id: Some("u-123".into()),
            ..Default::default()
        })
        .await;
    assert_eq!(ctx.user_message_id.as_deref(), Some("u-123"));
}

#[tokio::test]
async fn test_factory_threads_progress_tx_override_into_context() {
    // Phase 9 — progress forwarding. The engine builds one mpsc
    // channel per session, clones the tx into every `ToolUseContext`
    // built for that session, and drains the rx to `TuiOnlyEvent::
    // ToolProgress`. The factory is the one-place that wires the tx
    // into the context; a test-level override verifies the plumbing
    // without standing up a full engine.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<coco_tool_runtime::ToolProgress>();
    let ctx = factory(test_config())
        .build(ToolContextOverrides {
            progress_tx: Some(tx),
            ..Default::default()
        })
        .await;
    let ctx_tx = ctx
        .progress_tx
        .clone()
        .expect("progress_tx must propagate from overrides");
    ctx_tx
        .send(coco_tool_runtime::ToolProgress {
            tool_use_id: "abc".into(),
            parent_tool_use_id: None,
            data: serde_json::json!({"status": "running"}),
        })
        .unwrap();
    let got = rx
        .recv()
        .await
        .expect("drain side must receive tool progress");
    assert_eq!(got.tool_use_id, "abc");
}

#[tokio::test]
async fn test_factory_defaults_agent_handle_to_noop() {
    // Without `with_agent_handle`, the factory must hand out the
    // NoOp fallback so AgentTool invocations fail with a clean
    // "not available" error rather than panicking.
    let ctx = factory(test_config()).build(Default::default()).await;
    // Call a NoOp method — the NoOp impl returns Err, but the key
    // point is that the handle is installed (not a null pointer).
    let res = ctx.agent.send_message("any", "ping").await;
    assert!(res.is_err());
}

#[tokio::test]
async fn test_factory_installs_custom_agent_handle() {
    use async_trait::async_trait;
    use coco_tool_runtime::AgentHandle;
    use coco_tool_runtime::AgentSpawnRequest;
    use coco_tool_runtime::AgentSpawnResponse;

    struct MarkerHandle;
    #[async_trait]
    impl AgentHandle for MarkerHandle {
        async fn spawn_agent(
            &self,
            _request: AgentSpawnRequest,
        ) -> Result<AgentSpawnResponse, String> {
            Err("marker".into())
        }
        async fn send_message(&self, _to: &str, _content: &str) -> Result<String, String> {
            Ok("marker".into())
        }
        async fn create_team(
            &self,
            _request: coco_tool_runtime::CreateTeamRequest,
        ) -> Result<coco_tool_runtime::CreateTeamResult, String> {
            Err("marker".into())
        }
        async fn delete_team(&self) -> Result<String, String> {
            Err("marker".into())
        }
        // resume_agent uses the trait default impl.
        async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
            Err("marker".into())
        }
        async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
            Err("marker".into())
        }
    }

    let f = ToolContextFactory {
        agent_handle: Some(Arc::new(MarkerHandle)),
        ..factory(test_config())
    };
    let ctx = f.build(Default::default()).await;
    // `send_message` on the marker returns Ok("marker") — proves
    // the factory installed our handle, not the NoOp fallback.
    let res = ctx.agent.send_message("any", "ping").await;
    assert_eq!(res.as_deref().ok(), Some("marker"));
}

#[tokio::test]
async fn test_factory_propagates_cwd_override_from_config() {
    // Phase 6 Workstream C: QueryEngineConfig.cwd_override must
    // reach every ToolUseContext built by the factory so worktree-
    // isolated subagents see their worktree path on every tool call.
    use std::path::PathBuf;
    let override_path = PathBuf::from("/tmp/worktree-test-XYZ");
    let mut config = test_config();
    config.cwd_override = Some(override_path.clone());
    let ctx = factory(config).build(Default::default()).await;
    assert_eq!(
        ctx.cwd_override.as_ref(),
        Some(&override_path),
        "factory must install cwd_override on every ToolUseContext"
    );
}

#[tokio::test]
async fn test_factory_threads_allow_rules_from_base() {
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    let mut rules = std::collections::HashMap::new();
    rules.insert(
        PermissionRuleSource::UserSettings,
        vec![PermissionRule {
            source: PermissionRuleSource::UserSettings,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Read".into(),
                rule_content: None,
            },
        }],
    );
    let permissions = coco_types::LiveToolPermissionState {
        allow_rules: rules.clone(),
        ..Default::default()
    };
    let ctx = factory_with_base_rules(test_config(), permissions)
        .build(Default::default())
        .await;
    // PermissionRule doesn't impl PartialEq (foreign-crate type);
    // compare via JSON serialization for stable structural equality.
    assert_eq!(
        serde_json::to_string(&ctx.permission_context.allow_rules).unwrap(),
        serde_json::to_string(&rules).unwrap(),
        "factory must install allow_rules from the live base"
    );
}

#[tokio::test]
async fn test_factory_threads_deny_rules_from_base() {
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    let mut rules = std::collections::HashMap::new();
    rules.insert(
        PermissionRuleSource::PolicySettings,
        vec![PermissionRule {
            source: PermissionRuleSource::PolicySettings,
            behavior: PermissionBehavior::Deny,
            value: PermissionRuleValue {
                tool_pattern: "Bash".into(),
                rule_content: None,
            },
        }],
    );
    let permissions = coco_types::LiveToolPermissionState {
        deny_rules: rules.clone(),
        ..Default::default()
    };
    let ctx = factory_with_base_rules(test_config(), permissions)
        .build(Default::default())
        .await;
    assert_eq!(
        serde_json::to_string(&ctx.permission_context.deny_rules).unwrap(),
        serde_json::to_string(&rules).unwrap(),
    );
}

#[tokio::test]
async fn test_factory_merges_config_live_permission_rules_into_allow_rules() {
    // Regression for the mid-cycle "Always Allow" staleness bug: a rule
    // pushed into the shared `live_permission_rules` overlay must surface in
    // the built context's allow_rules — under its own source — so the
    // in-flight engine (whose base config is a frozen snapshot) observes the
    // approval the SAME cycle. This is what lets a fresh `Edit(...)` grant
    // satisfy a same-cycle Read via the "edit access implies read" branch.
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    let mut config = test_config();
    let edit_rule = PermissionRule {
        source: PermissionRuleSource::LocalSettings,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: "Edit".into(),
            rule_content: Some("//tmp/b/**".into()),
        },
    };
    config.live_permission_rules = Some(std::sync::Arc::new(tokio::sync::RwLock::new(vec![
        edit_rule.clone(),
    ])));
    let ctx = factory(config).build(Default::default()).await;
    let local = ctx
        .permission_context
        .allow_rules
        .get(&PermissionRuleSource::LocalSettings)
        .expect("overlay rule must land under its own source");
    assert_eq!(
        serde_json::to_string(local).unwrap(),
        serde_json::to_string(&vec![edit_rule]).unwrap(),
        "factory must merge config.live_permission_rules into allow_rules \
         preserving the rule's source",
    );
}

#[tokio::test]
async fn test_factory_reads_live_app_state_base_and_sees_midcycle_mutation() {
    // S2: the factory sources allow/deny/ask from the shared live
    // `ToolAppState.permissions` base (not the frozen config), and a mutation
    // between two `build()` calls is visible on the second — the TS
    // read-through property that makes mid-cycle "Always Allow" take effect.
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    use coco_types::ToolAppState;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    let allow = PermissionRule {
        source: PermissionRuleSource::LocalSettings,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: "Edit".into(),
            rule_content: Some("//tmp/b/**".into()),
        },
    };
    app_state
        .write()
        .await
        .permissions
        .allow_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(allow);

    let f = ToolContextFactory {
        app_state: Some(app_state.clone()),
        ..factory(test_config())
    };
    let ctx1 = f.build(Default::default()).await;
    assert!(
        ctx1.permission_context
            .allow_rules
            .contains_key(&PermissionRuleSource::LocalSettings),
        "factory must source allow rules from the live app_state base",
    );

    // Mutate the shared base mid-cycle, then rebuild: the new deny rule must be
    // visible without rebuilding the engine (read-through each batch).
    app_state
        .write()
        .await
        .permissions
        .deny_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(PermissionRule {
            source: PermissionRuleSource::LocalSettings,
            behavior: PermissionBehavior::Deny,
            value: PermissionRuleValue {
                tool_pattern: "Read".into(),
                rule_content: Some("//etc/**".into()),
            },
        });
    let ctx2 = f.build(Default::default()).await;
    assert!(
        ctx2.permission_context
            .deny_rules
            .contains_key(&PermissionRuleSource::LocalSettings),
        "a mid-cycle base mutation must be visible on the next build()",
    );
}

#[tokio::test]
async fn test_subagent_inherits_parent_deny_via_shared_base() {
    // S3 SECURITY FIX: a subagent shares the parent's app_state Arc, so with
    // rules in the live base it read-through-inherits the parent's DENY rules
    // (TS createSubagentContext). Closes the confirmed read-bypass where a
    // subagent could read a file the parent `deny Read`-ed.
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    use coco_types::ToolAppState;

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    app_state
        .write()
        .await
        .permissions
        .deny_rules
        .entry(PermissionRuleSource::LocalSettings)
        .or_default()
        .push(PermissionRule {
            source: PermissionRuleSource::LocalSettings,
            behavior: PermissionBehavior::Deny,
            value: PermissionRuleValue {
                tool_pattern: "Read".into(),
                rule_content: Some("//proj/secrets/**".into()),
            },
        });

    // Subagent config: permission_derivation = Some (read-through derivation),
    // empty config base rules (the bug's original empty maps).
    let subagent_cfg = QueryEngineConfig {
        agent_id: Some("agent-1".into()),
        permission_derivation: Some(crate::config::PermissionDerivation::default()),
        ..test_config()
    };
    let f = ToolContextFactory {
        app_state: Some(app_state.clone()),
        ..factory(subagent_cfg)
    };
    let ctx = f.build(Default::default()).await;
    assert!(
        ctx.permission_context
            .deny_rules
            .contains_key(&PermissionRuleSource::LocalSettings),
        "subagent must inherit the parent's deny rules via the shared live base",
    );
}

#[tokio::test]
async fn test_subagent_allowed_tools_replace_on_restrict() {
    // S3 TS parity (runAgent.ts:469-479): when a subagent is restricted via
    // allowed_tools, the derived ALLOW = parent CliArg-source only + the allowed
    // tools as Session-source; the parent's other allow sources are dropped.
    // deny is always still inherited.
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;
    use coco_types::ToolAppState;

    fn rule(
        source: PermissionRuleSource,
        behavior: PermissionBehavior,
        pat: &str,
    ) -> PermissionRule {
        PermissionRule {
            source,
            behavior,
            value: PermissionRuleValue {
                tool_pattern: pat.into(),
                rule_content: None,
            },
        }
    }

    let app_state = Arc::new(RwLock::new(ToolAppState::default()));
    {
        let mut g = app_state.write().await;
        g.permissions
            .allow_rules
            .entry(PermissionRuleSource::CliArg)
            .or_default()
            .push(rule(
                PermissionRuleSource::CliArg,
                PermissionBehavior::Allow,
                "Glob",
            ));
        g.permissions
            .allow_rules
            .entry(PermissionRuleSource::LocalSettings)
            .or_default()
            .push(rule(
                PermissionRuleSource::LocalSettings,
                PermissionBehavior::Allow,
                "Bash",
            ));
        g.permissions
            .deny_rules
            .entry(PermissionRuleSource::LocalSettings)
            .or_default()
            .push(rule(
                PermissionRuleSource::LocalSettings,
                PermissionBehavior::Deny,
                "Write",
            ));
    }

    let subagent_cfg = QueryEngineConfig {
        agent_id: Some("agent-1".into()),
        permission_derivation: Some(crate::config::PermissionDerivation {
            allowed_tools_replace: Some(vec![rule(
                PermissionRuleSource::Session,
                PermissionBehavior::Allow,
                "Read",
            )]),
            ..Default::default()
        }),
        ..test_config()
    };
    let f = ToolContextFactory {
        app_state: Some(app_state.clone()),
        ..factory(subagent_cfg)
    };
    let allow = f
        .build(Default::default())
        .await
        .permission_context
        .allow_rules;

    assert!(
        allow.contains_key(&PermissionRuleSource::CliArg),
        "parent CliArg allow is preserved on restrict",
    );
    assert!(
        allow.contains_key(&PermissionRuleSource::Session),
        "allowed_tools injected as Session allow",
    );
    assert!(
        !allow.contains_key(&PermissionRuleSource::LocalSettings),
        "parent's other allow sources are dropped on restrict",
    );
    let deny = f
        .build(Default::default())
        .await
        .permission_context
        .deny_rules;
    assert!(
        deny.contains_key(&PermissionRuleSource::LocalSettings),
        "deny is always inherited regardless of allowed_tools",
    );
}

#[tokio::test]
async fn test_factory_cwd_override_none_when_config_unset() {
    // Baseline: no override in config → no override on context.
    // Guards against a stray default slipping in.
    let ctx = factory(test_config()).build(Default::default()).await;
    assert!(
        ctx.cwd_override.is_none(),
        "factory must not synthesize a cwd_override when config has none"
    );
}

#[tokio::test]
async fn test_factory_defaults_skill_handle_to_noop_unavailable() {
    // NoOpSkillHandle returns `Unavailable` — verifies the factory
    // installs it when no real runtime is wired.
    let ctx = factory(test_config()).build(Default::default()).await;
    let err = ctx
        .skill
        .invoke_skill(
            "any",
            "",
            coco_tool_runtime::SubagentInheritance::default(),
            coco_tool_runtime::SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        coco_tool_runtime::SkillInvocationError::Unavailable { .. }
    ));
}

// ── Live Command-source rule merge tests ──
//
// Verifies the `engine.live_command_rules` Arc threaded into the
// factory is read at every `build()` and folded into
// `permission_context.allow_rules[Command]`. `alwaysAllowRules.command`
// is read at each permission check; per-engine = per-user-msg scoping
// comes from the engine's fresh-per-turn lifecycle
// (see `engine_live_rules` module docs).

fn skill_cmd_rule(tool_pattern: &str) -> coco_types::PermissionRule {
    coco_types::PermissionRule {
        source: coco_types::PermissionRuleSource::Command,
        behavior: coco_types::PermissionBehavior::Allow,
        value: coco_types::PermissionRuleValue {
            tool_pattern: tool_pattern.into(),
            rule_content: None,
        },
    }
}

fn session_rule(
    behavior: coco_types::PermissionBehavior,
    tool_pattern: &str,
) -> coco_types::PermissionRule {
    coco_types::PermissionRule {
        source: coco_types::PermissionRuleSource::Session,
        behavior,
        value: coco_types::PermissionRuleValue {
            tool_pattern: tool_pattern.into(),
            rule_content: None,
        },
    }
}

#[tokio::test]
async fn test_factory_returns_base_allow_rules_when_live_rules_empty() {
    // Zero-clone fast path: when the live store is empty, the
    // factory must hand back the live base allow_rules verbatim
    // (no Command entry inserted). This is the common case — the
    // overwhelming majority of turns have no skill-emitted rules.
    let mut base_rules = std::collections::HashMap::new();
    base_rules.insert(
        coco_types::PermissionRuleSource::UserSettings,
        vec![coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::UserSettings,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: "Read".into(),
                rule_content: None,
            },
        }],
    );
    let permissions = coco_types::LiveToolPermissionState {
        allow_rules: base_rules.clone(),
        ..Default::default()
    };
    let ctx = factory_with_base_rules(test_config(), permissions)
        .build(Default::default())
        .await;
    // PermissionRule doesn't impl PartialEq; compare via JSON roundtrip.
    assert_eq!(
        serde_json::to_string(&ctx.permission_context.allow_rules).unwrap(),
        serde_json::to_string(&base_rules).unwrap(),
        "fast path must hand back base allow_rules verbatim"
    );
    // No Command source ever materialised because we never inserted one.
    assert!(
        !ctx.permission_context
            .allow_rules
            .contains_key(&coco_types::PermissionRuleSource::Command)
    );
}

#[tokio::test]
async fn test_factory_merges_live_rules_into_command_source() {
    // Skill emitted a rule earlier this user message — factory.build
    // for the next batch must surface it under the Command source so
    // the evaluator sees it.
    let config = test_config();
    let store: Arc<RwLock<Vec<coco_types::PermissionRule>>> =
        Arc::new(RwLock::new(vec![skill_cmd_rule("Read")]));
    let ctx = factory_with_live_rules(config, store.clone())
        .build(Default::default())
        .await;
    let cmd_rules = ctx
        .permission_context
        .allow_rules
        .get(&coco_types::PermissionRuleSource::Command)
        .expect("Command source should be populated from live rules");
    assert_eq!(cmd_rules.len(), 1);
    assert_eq!(cmd_rules[0].value.tool_pattern, "Read");
}

#[tokio::test]
async fn test_factory_cross_batch_propagation_within_same_arc() {
    // Same Arc shared with the (hypothetical) engine + handle:
    // batch 1's `build()` sees `[Read]`, then a "tool emission"
    // appends `[Edit]`, batch 2's `build()` sees both. This is the
    // cross-turn-within-user-msg propagation path.
    let config = test_config();
    let store: Arc<RwLock<Vec<coco_types::PermissionRule>>> =
        Arc::new(RwLock::new(vec![skill_cmd_rule("Read")]));
    let factory = factory_with_live_rules(config, store.clone());

    let ctx1 = factory.build(Default::default()).await;
    let cmd1 = ctx1
        .permission_context
        .allow_rules
        .get(&coco_types::PermissionRuleSource::Command)
        .expect("batch 1 should see Read");
    assert_eq!(cmd1.len(), 1);

    // Simulate a tool emission between batches.
    store.write().await.push(skill_cmd_rule("Edit"));

    let ctx2 = factory.build(Default::default()).await;
    let cmd2 = ctx2
        .permission_context
        .allow_rules
        .get(&coco_types::PermissionRuleSource::Command)
        .expect("batch 2 should see both rules");
    let patterns: Vec<&str> = cmd2.iter().map(|r| r.value.tool_pattern.as_str()).collect();
    assert_eq!(patterns, vec!["Read", "Edit"]);
}

#[tokio::test]
async fn test_factory_isolates_per_arc_engines() {
    // Two factories with two independent Arc-stores ≡ two engines
    // (= two user messages, or main + subagent). A write into one
    // must NOT be visible through the other.
    let config_a = test_config();
    let config_b = test_config();
    let store_a: Arc<RwLock<Vec<coco_types::PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));
    let store_b: Arc<RwLock<Vec<coco_types::PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));

    store_a.write().await.push(skill_cmd_rule("Read"));

    let ctx_a = factory_with_live_rules(config_a, store_a)
        .build(Default::default())
        .await;
    let ctx_b = factory_with_live_rules(config_b, store_b)
        .build(Default::default())
        .await;

    assert!(
        ctx_a
            .permission_context
            .allow_rules
            .contains_key(&coco_types::PermissionRuleSource::Command)
    );
    assert!(
        !ctx_b
            .permission_context
            .allow_rules
            .contains_key(&coco_types::PermissionRuleSource::Command)
    );
}

#[tokio::test]
async fn test_factory_preserves_base_command_rules_when_merging() {
    // If the user has set Command-source rules at the config layer
    // (e.g. via CLI `--allow Command:Read`, though uncommon), the
    // factory must append live rules to those rather than replace
    // the base.
    let mut base_allow = std::collections::HashMap::new();
    base_allow.insert(
        coco_types::PermissionRuleSource::Command,
        vec![skill_cmd_rule("Glob")],
    );
    let permissions = coco_types::LiveToolPermissionState {
        allow_rules: base_allow,
        ..Default::default()
    };

    let store: Arc<RwLock<Vec<coco_types::PermissionRule>>> =
        Arc::new(RwLock::new(vec![skill_cmd_rule("Read")]));
    let ctx = ToolContextFactory {
        live_command_rules: store,
        ..factory_with_base_rules(test_config(), permissions)
    }
    .build(Default::default())
    .await;
    let cmd_rules = ctx
        .permission_context
        .allow_rules
        .get(&coco_types::PermissionRuleSource::Command)
        .expect("Command source should retain base + live entries");
    let patterns: Vec<&str> = cmd_rules
        .iter()
        .map(|r| r.value.tool_pattern.as_str())
        .collect();
    assert_eq!(patterns, vec!["Glob", "Read"]);
}

#[tokio::test]
async fn test_factory_merges_live_permission_rules_by_behavior() {
    let mut config = test_config();
    let live_rules = Arc::new(RwLock::new(vec![
        session_rule(coco_types::PermissionBehavior::Allow, "Read"),
        session_rule(coco_types::PermissionBehavior::Deny, "Bash"),
        session_rule(coco_types::PermissionBehavior::Ask, "Edit"),
    ]));
    config.live_permission_rules = Some(live_rules);

    let ctx = factory(config).build(Default::default()).await;

    assert_eq!(
        ctx.permission_context.allow_rules[&coco_types::PermissionRuleSource::Session][0]
            .value
            .tool_pattern,
        "Read"
    );
    assert_eq!(
        ctx.permission_context.deny_rules[&coco_types::PermissionRuleSource::Session][0]
            .value
            .tool_pattern,
        "Bash"
    );
    assert_eq!(
        ctx.permission_context.ask_rules[&coco_types::PermissionRuleSource::Session][0]
            .value
            .tool_pattern,
        "Edit"
    );
}

#[tokio::test]
async fn test_factory_uses_live_permission_mode_override() {
    let mut config = test_config();
    config.permission_mode = PermissionMode::Default;
    config.live_permission_mode = Some(Arc::new(RwLock::new(PermissionMode::AcceptEdits)));

    let ctx = factory(config).build(Default::default()).await;

    assert_eq!(ctx.permission_context.mode, PermissionMode::AcceptEdits);
}

#[tokio::test]
async fn test_factory_threads_messages_snapshot() {
    // Post-budget messages snapshot from `build_prompt` reaches
    // `ctx.messages` per turn. Without this, AgentTool fork-mode's
    // `is_in_fork_child` recursion guard could never trigger and
    // `parent_messages` was always empty.
    let parent_msg = Arc::new(coco_messages::create_user_message("parent turn 1"));
    let snapshot = Arc::new(vec![parent_msg.clone()]);
    let ctx = factory(test_config())
        .build(ToolContextOverrides {
            messages_snapshot: Some(snapshot.clone()),
            ..Default::default()
        })
        .await;
    assert_eq!(ctx.messages.len(), 1);
    // Same Arc — no clone at the factory seam.
    assert!(Arc::ptr_eq(&ctx.messages, &snapshot));
    assert!(Arc::ptr_eq(&ctx.messages[0], &parent_msg));
}

#[tokio::test]
async fn test_factory_defaults_messages_to_empty_when_no_snapshot() {
    // Test stubs / pre-first-turn paths build a ctx without a
    // snapshot. Factory must fall back to an empty `Arc<Vec<…>>` so
    // existing read sites (e.g. `is_in_fork_child` on an empty vec)
    // see deterministic empty state, not panic.
    let ctx = factory(test_config()).build(Default::default()).await;
    assert!(ctx.messages.is_empty());
}

#[tokio::test]
async fn test_factory_messages_snapshot_supports_fork_recursion_guard() {
    // With the snapshot threaded, `coco_subagent::is_in_fork_child`
    // correctly detects a parent history containing the
    // `<fork-boilerplate>` tag. Without the per-turn injection this
    // returned `false` on the empty default vec and fork-of-fork was
    // silently allowed.
    let directive = coco_subagent::build_fork_child_message("ignored");
    let in_fork_msg = Arc::new(coco_messages::create_user_message(&directive));
    let plain_msg = Arc::new(coco_messages::create_user_message("hello"));

    let ctx_in_fork = factory(test_config())
        .build(ToolContextOverrides {
            messages_snapshot: Some(Arc::new(vec![in_fork_msg])),
            ..Default::default()
        })
        .await;
    let view_in_fork: Vec<_> = ctx_in_fork.messages.iter().cloned().collect();
    assert!(
        coco_subagent::is_in_fork_child(&view_in_fork),
        "recursion guard must fire when parent history carries the boilerplate tag",
    );

    let ctx_plain = factory(test_config())
        .build(ToolContextOverrides {
            messages_snapshot: Some(Arc::new(vec![plain_msg])),
            ..Default::default()
        })
        .await;
    let view_plain: Vec<_> = ctx_plain.messages.iter().cloned().collect();
    assert!(
        !coco_subagent::is_in_fork_child(&view_plain),
        "normal user history must NOT trip the guard",
    );
}
