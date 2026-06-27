//! End-to-end scoping tests for the per-engine `live_command_rules`
//! Arc.
//!
//! These tests pin the lifecycle invariants for the per-engine
//! `appState.alwaysAllowRules.command` slot:
//!
//! - **Engine = 1 user message**: every `QueryEngine::new` allocates a
//!   fresh empty Arc. There is no cross-engine sharing.
//! - **Handle ↔ engine ↔ factory share one Arc**: a write through
//!   `engine.permission_rule_handle.apply_updates` is observable via
//!   the factory's batch-time merge AND via `engine.live_command_rules`
//!   directly.
//! - **Drop ⇒ release**: when the engine drops, the Arc's strong count
//!   returns to the handle's only reference (or 0 once the handle is
//!   also dropped).
//!
//! These tests use the same `StubModel` harness as
//! `engine_attachments.test.rs` — no real LLM call, just engine
//! construction + handle/factory plumbing.

use std::sync::Arc;

use coco_tool_runtime::ToolRegistry;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use crate::config::QueryEngineConfig;
use crate::engine::QueryEngine;

/// Reuse the `StubModel` shape from `engine_attachments.test.rs`.
struct StubModel;

#[async_trait::async_trait]
impl coco_inference::LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }
    fn model_id(&self) -> &str {
        "stub"
    }
    async fn do_generate(
        &self,
        _options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelGenerateResult, coco_inference::AISdkError> {
        Ok(coco_inference::LanguageModelGenerateResult {
            content: vec![coco_llm_types::AssistantContentPart::Text(
                coco_llm_types::TextPart {
                    text: "".into(),
                    provider_metadata: None,
                },
            )],
            usage: coco_llm_types::Usage::new(0, 0),
            finish_reason: coco_llm_types::FinishReason::new(coco_llm_types::StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelStreamResult, coco_inference::AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

fn make_engine() -> QueryEngine {
    let model = Arc::new(StubModel);
    let client = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(ToolRegistry::new());
    let cancel = CancellationToken::new();
    QueryEngine::new(QueryEngineConfig::default(), client, tools, cancel, None)
}

fn skill_cmd_rule(tool_pattern: &str) -> PermissionRule {
    PermissionRule {
        source: PermissionRuleSource::Command,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool_pattern.into(),
            rule_content: None,
        },
    }
}

#[tokio::test]
async fn new_engine_starts_with_empty_live_rules() {
    // A fresh engine starts with an empty
    // `appState.alwaysAllowRules.command` slot.
    let engine = make_engine();
    assert!(engine.live_command_rules.read().await.is_empty());
}

#[tokio::test]
async fn handle_writes_visible_through_engine_arc() {
    // The engine's auto-installed `EngineLiveRulesHandle` shares the
    // exact same Arc with `engine.live_command_rules`. Writing through
    // the handle MUST be observable through the engine field — that's
    // the contract the factory's batch-time merge relies on.
    let engine = make_engine();
    engine
        .permission_rule_handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![skill_cmd_rule("Read")],
            destination: PermissionUpdateDestination::Command,
        }])
        .await;
    let guard = engine.live_command_rules.read().await;
    assert_eq!(guard.len(), 1);
    assert_eq!(guard[0].value.tool_pattern, "Read");
}

#[tokio::test]
async fn factory_sees_handle_writes_within_same_engine() {
    // Cross-turn-within-user-msg propagation: turn 1's tool emits a
    // rule via the handle; turn 2's `factory.build()` MUST observe it
    // in `permission_context.allow_rules[Command]`. This is what
    // makes inline skills' `allowed-tools` honored on the very next
    // turn (`getAppState` reads same closure-captured ref).
    let engine = make_engine();
    engine
        .permission_rule_handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![skill_cmd_rule("Edit")],
            destination: PermissionUpdateDestination::Command,
        }])
        .await;
    let ctx = engine
        .tool_context_factory(None)
        .build(Default::default())
        .await;
    let cmd_rules = ctx
        .permission_context
        .allow_rules
        .get(&PermissionRuleSource::Command)
        .expect("factory must merge live rules into Command source");
    assert_eq!(cmd_rules.len(), 1);
    assert_eq!(cmd_rules[0].value.tool_pattern, "Edit");
}

#[tokio::test]
async fn two_engines_have_independent_arcs() {
    // Cross-user-msg isolation: each user message gets a fresh
    // `QueryEngine` via `SessionRuntime::build_engine`, so each gets
    // its own Arc. Rules emitted in engine_a MUST NOT leak into
    // engine_b. This also covers the subagent-fork case where the
    // subagent's `build_engine_from_config` allocates a separate
    // engine = separate Arc (no NoOp override needed).
    let engine_a = make_engine();
    let engine_b = make_engine();
    engine_a
        .permission_rule_handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![skill_cmd_rule("Read")],
            destination: PermissionUpdateDestination::Command,
        }])
        .await;
    assert_eq!(engine_a.live_command_rules.read().await.len(), 1);
    assert!(engine_b.live_command_rules.read().await.is_empty());
}

#[tokio::test]
async fn dropping_engine_releases_arc() {
    // After the engine drops, only the handle's clone keeps the Arc
    // alive. This is the "drop on user msg return" guarantee — the
    // store cannot outlive the engine that owns it (modulo a handle
    // ref that was extracted, which is by design and explicitly
    // kept alive by the caller).
    let engine = make_engine();
    let arc_clone = engine.live_command_rules.clone();
    let handle_clone = engine.permission_rule_handle.clone();
    assert_eq!(Arc::strong_count(&arc_clone), 3); // engine + handle + clone
    drop(engine);
    // Handle still holds an Arc internally + our explicit clone.
    assert_eq!(Arc::strong_count(&arc_clone), 2);
    drop(handle_clone);
    // Only our test clone remains.
    assert_eq!(Arc::strong_count(&arc_clone), 1);
}

#[tokio::test]
async fn with_permission_rule_handle_override_redirects_writes_only() {
    // Documented contract: `with_permission_rule_handle` swaps the
    // WRITE side only. The factory still reads the engine's original
    // `live_command_rules` Arc, so installing a `NoOp` makes
    // emissions go nowhere visible — useful for tests that want to
    // exercise the rest of the executor without observable state
    // mutation.
    let engine = make_engine()
        .with_permission_rule_handle(Arc::new(coco_tool_runtime::NoOpPermissionRuleHandle));
    engine
        .permission_rule_handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![skill_cmd_rule("Read")],
            destination: PermissionUpdateDestination::Command,
        }])
        .await;
    // Write went to /dev/null — the engine's Arc is untouched.
    assert!(engine.live_command_rules.read().await.is_empty());
    let ctx = engine
        .tool_context_factory(None)
        .build(Default::default())
        .await;
    assert!(
        !ctx.permission_context
            .allow_rules
            .contains_key(&PermissionRuleSource::Command)
    );
}

#[tokio::test]
async fn non_command_destination_writes_dropped() {
    // Disk-persisting destinations (UserSettings / ProjectSettings /
    // LocalSettings) and the session-config-mutating ones
    // (Session / CliArg) are NOT handled by the per-engine handle.
    // Skills today only emit Command, so any other variant arriving
    // here is silently dropped. This test pins that contract.
    let engine = make_engine();
    engine
        .permission_rule_handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![skill_cmd_rule("Read")],
            destination: PermissionUpdateDestination::UserSettings,
        }])
        .await;
    assert!(engine.live_command_rules.read().await.is_empty());
}
