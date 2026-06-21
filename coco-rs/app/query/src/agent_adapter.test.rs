use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use coco_tool_runtime::AgentQueryConfig;
use coco_tool_runtime::AgentQueryEngine;

#[test]
fn inherited_read_dirs_map_to_session_additional_dirs() {
    // TS `createSubagentContext` parity: the parent's read-scope dirs become
    // the child's permission `additional_dirs`, so an isolated-worktree
    // subagent reads the parent project without a prompt.
    let dirs = vec!["/repo".to_string(), "/repo/extra".to_string()];
    let map = super::inherited_read_dirs_to_additional_dirs(&dirs);
    assert_eq!(map.len(), 2);
    let entry = map.get("/repo").expect("parent cwd present");
    assert_eq!(entry.path, "/repo");
    assert_eq!(entry.source, coco_types::WorkingDirectorySource::Session);
    // Empty in → empty out (non-worktree / fork spawns add nothing).
    assert!(super::inherited_read_dirs_to_additional_dirs(&[]).is_empty());
}

#[test]
fn test_agent_query_config_fork_context_messages_field_round_trips() {
    // Lock in that fork_context_messages serializes through the
    // boundary — `Vec<Arc<Message>>` round-trips via the serde `rc`
    // feature (Arc<T> serializes transparently as T, deserializes as
    // a fresh Arc).
    let parent_msg = Arc::new(coco_messages::create_user_message("parent turn 1"));
    let cfg = AgentQueryConfig {
        system_prompt: "s".into(),
        max_turns: Some(1),
        preserve_tool_use_results: true,
        fork_context_messages: vec![parent_msg],
        ..Default::default()
    };
    let s = serde_json::to_string(&cfg).unwrap();
    let back: AgentQueryConfig = serde_json::from_str(&s).unwrap();
    assert!(back.preserve_tool_use_results);
    assert_eq!(back.fork_context_messages.len(), 1);
    assert!(matches!(
        back.fork_context_messages[0].as_ref(),
        coco_messages::Message::User(_)
    ));
}

#[tokio::test]
async fn test_no_op_engine_returns_error() {
    let engine = coco_tool_runtime::NoOpAgentQueryEngine;
    let config = AgentQueryConfig {
        system_prompt: "test".into(),
        max_turns: Some(1),
        ..Default::default()
    };
    let result = engine.execute_query("hello", config).await;
    assert!(result.is_err());
}

/// Test-only factory variant that observes the role argument and
/// returns an error via a synthetic `QueryEngine`-less path. We
/// can't return a `QueryEngine` here without building a whole
/// runtime registry + `ToolRegistry`; instead we use
/// [`std::panic::catch_unwind`] to trap a controlled panic INSIDE
/// the factory closure, leaving the assertion on `observed` the
/// durable check. This avoids depending on tokio spawn semantics
/// for panic propagation.
fn role_observer() -> (
    Arc<std::sync::Mutex<coco_types::LlmModelSelection>>,
    Arc<AtomicU32>,
) {
    (
        Arc::new(std::sync::Mutex::new(
            coco_types::LlmModelSelection::InheritMain,
        )),
        Arc::new(AtomicU32::new(0)),
    )
}

#[tokio::test]
async fn test_adapter_threads_role_selection_to_factory() {
    // Contract: typed `AgentQueryConfig.model_selection` flows through the
    // factory unchanged so downstream code can resolve the right runtime.
    use super::QueryEngineAdapter;
    use super::QueryEngineFactory;

    let (observed, calls) = role_observer();
    let observed_c = observed.clone();
    let calls_c = calls.clone();

    let factory: QueryEngineFactory = Arc::new(move |_cfg, role, _cancel| {
        let observed_c = observed_c.clone();
        let calls_c = calls_c.clone();
        Box::pin(async move {
            *observed_c.lock().unwrap() = role;
            calls_c.fetch_add(1, Ordering::SeqCst);
            // Controlled abort: the factory MUST be driven by adapter
            // in production. We use `resume_unwind` with a sentinel so
            // `catch_unwind` in the driving test traps it cleanly.
            std::panic::resume_unwind(Box::new("observed"));
        })
    });
    let adapter = QueryEngineAdapter::new(factory);

    let cfg = AgentQueryConfig {
        system_prompt: "s".into(),
        max_turns: Some(1),
        model_selection: coco_types::LlmModelSelection::Role {
            role: coco_types::ModelRole::Explore,
        },
        ..Default::default()
    };

    // `catch_unwind` on an async path: wrap `futures::executor` or
    // spawn-and-join. tokio's JoinError carries the panic payload
    // and is the standard test pattern; kept here for async
    // compatibility.
    let handle = tokio::task::spawn(async move { adapter.execute_query("hello", cfg).await });
    let join_err = handle.await.expect_err("factory panic must bubble up");
    assert!(join_err.is_panic());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "factory called exactly once",
    );
    assert_eq!(
        *observed.lock().unwrap(),
        coco_types::LlmModelSelection::Role {
            role: coco_types::ModelRole::Explore,
        },
        "adapter must forward typed role selection",
    );
}

#[tokio::test]
async fn test_adapter_threads_provider_model_selection_to_factory() {
    use super::QueryEngineAdapter;
    use super::QueryEngineFactory;

    let (observed, calls) = role_observer();
    let observed_c = observed.clone();
    let calls_c = calls.clone();

    let factory: QueryEngineFactory = Arc::new(move |_cfg, selection, _cancel| {
        let observed_c = observed_c.clone();
        let calls_c = calls_c.clone();
        Box::pin(async move {
            *observed_c.lock().unwrap() = selection;
            calls_c.fetch_add(1, Ordering::SeqCst);
            std::panic::resume_unwind(Box::new("observed"));
        })
    });
    let adapter = QueryEngineAdapter::new(factory);

    let cfg = AgentQueryConfig {
        system_prompt: "s".into(),
        max_turns: Some(1),
        model_selection: coco_types::LlmModelSelection::ExplicitWithFallbackRole {
            primary: coco_types::ProviderModelSelection {
                provider: "openai".into(),
                model_id: "gpt-5.4".into(),
            },
            fallback_role: coco_types::ModelRole::Review,
        },
        ..Default::default()
    };

    let handle = tokio::task::spawn(async move { adapter.execute_query("hello", cfg).await });
    let join_err = handle.await.expect_err("factory panic must bubble up");
    assert!(join_err.is_panic());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *observed.lock().unwrap(),
        coco_types::LlmModelSelection::ExplicitWithFallbackRole {
            primary: coco_types::ProviderModelSelection {
                provider: "openai".into(),
                model_id: "gpt-5.4".into(),
            },
            fallback_role: coco_types::ModelRole::Review,
        },
    );
}

#[tokio::test]
async fn test_adapter_parses_effort_string_into_thinking_level() {
    // Contract: `AgentQueryConfig.effort = Some("high")` must arrive
    // at the engine's `QueryEngineConfig.thinking_level` as a
    // `ThinkingLevel` whose effort is `High`. An unrecognized string
    // degrades to `None` (model's `default_thinking_level` then wins).
    use super::QueryEngineAdapter;
    use super::QueryEngineFactory;

    let observed: Arc<std::sync::Mutex<Option<coco_types::ThinkingLevel>>> =
        Arc::new(std::sync::Mutex::new(None));
    let observed_c = observed.clone();

    let factory: QueryEngineFactory = Arc::new(move |cfg, _role, _cancel| {
        let observed_c = observed_c.clone();
        Box::pin(async move {
            *observed_c.lock().unwrap() = cfg.thinking_level;
            std::panic::resume_unwind(Box::new("observed"));
        })
    });
    let adapter = QueryEngineAdapter::new(factory);

    let cfg = AgentQueryConfig {
        system_prompt: "s".into(),
        max_turns: Some(1),
        effort: Some(coco_types::ReasoningEffort::High),
        ..Default::default()
    };

    let handle = tokio::task::spawn(async move { adapter.execute_query("hello", cfg).await });
    let _ = handle.await.expect_err("factory panic must bubble up");
    let captured = observed.lock().unwrap().clone();
    assert!(
        captured.is_some(),
        "effort=High must produce a ThinkingLevel"
    );
    assert_eq!(
        captured.unwrap().effort,
        coco_types::ReasoningEffort::High,
        "High propagates through",
    );
}

// Note: the previous `test_adapter_unknown_effort_degrades_to_none` and
// `test_adapter_max_effort_alias_maps_to_xhigh` tests exercised the
// adapter's STRING parsing path (config.effort: Option<String> →
// ThinkingLevel via str::parse). That path is gone — the type system
// now enforces a valid `ReasoningEffort` at the AgentQueryConfig
// boundary, so "unknown" and "max" string aliases are no longer the
// adapter's concern. Alias handling (`max` → `XHigh`) is covered by
// `common/types/src/thinking.test.rs::test_reasoning_effort_aliases`.

#[tokio::test]
async fn test_adapter_xhigh_effort_propagates() {
    use super::QueryEngineAdapter;
    use super::QueryEngineFactory;

    let observed: Arc<std::sync::Mutex<Option<coco_types::ThinkingLevel>>> =
        Arc::new(std::sync::Mutex::new(None));
    let observed_c = observed.clone();

    let factory: QueryEngineFactory = Arc::new(move |cfg, _role, _cancel| {
        let observed_c = observed_c.clone();
        Box::pin(async move {
            *observed_c.lock().unwrap() = cfg.thinking_level;
            std::panic::resume_unwind(Box::new("observed"));
        })
    });
    let adapter = QueryEngineAdapter::new(factory);

    let cfg = AgentQueryConfig {
        system_prompt: "s".into(),
        max_turns: Some(1),
        effort: Some(coco_types::ReasoningEffort::XHigh),
        ..Default::default()
    };

    let handle = tokio::task::spawn(async move { adapter.execute_query("hello", cfg).await });
    let _ = handle.await.expect_err("factory panic must bubble up");
    assert_eq!(
        observed.lock().unwrap().as_ref().map(|t| t.effort),
        Some(coco_types::ReasoningEffort::XHigh),
    );
}

#[tokio::test]
async fn test_adapter_defers_to_factory_default_when_role_none() {
    use super::QueryEngineAdapter;
    use super::QueryEngineFactory;

    // Seed with a sentinel other than InheritMain so the observer proves
    // the factory saw exactly the default selection.
    let observed: Arc<std::sync::Mutex<coco_types::LlmModelSelection>> =
        Arc::new(std::sync::Mutex::new(coco_types::LlmModelSelection::Role {
            role: coco_types::ModelRole::Memory,
        }));
    let observed_c = observed.clone();

    let factory: QueryEngineFactory = Arc::new(move |_cfg, role, _cancel| {
        let observed_c = observed_c.clone();
        Box::pin(async move {
            *observed_c.lock().unwrap() = role;
            std::panic::resume_unwind(Box::new("observed"));
        })
    });
    let adapter = QueryEngineAdapter::new(factory);

    let cfg = AgentQueryConfig {
        system_prompt: "s".into(),
        max_turns: Some(1),
        ..Default::default()
    };

    let handle = tokio::task::spawn(async move { adapter.execute_query("hello", cfg).await });
    let join_err = handle.await.expect_err("factory panic must bubble up");
    assert!(join_err.is_panic());
    assert_eq!(
        *observed.lock().unwrap(),
        coco_types::LlmModelSelection::InheritMain,
        "InheritMain must flow verbatim so the factory applies its default",
    );
}
