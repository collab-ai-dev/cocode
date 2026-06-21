//! Per-engine `EngineLiveRulesHandle` unit tests.
//!
//! Covers the contract: only `AddRules{destination: Command}` writes
//! through; everything else drops with a `tracing::debug!`. Lifetime
//! scoping (per-engine = per-user-msg, subagent isolation) is exercised
//! end-to-end in `app/query/src/engine_live_rules_scoping.test.rs` and
//! `app/query/src/skill_runtime.test.rs`.

use super::EngineLiveRulesHandle;
use coco_tool_runtime::PermissionRuleHandle;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use std::sync::Arc;
use tokio::sync::RwLock;

fn cmd_allow_rule(tool: &str) -> PermissionRule {
    PermissionRule {
        source: PermissionRuleSource::Command,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: None,
        },
    }
}

#[tokio::test]
async fn add_rules_command_destination_writes_through() {
    let store: Arc<RwLock<Vec<PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));
    let handle = EngineLiveRulesHandle::new(store.clone());

    handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![cmd_allow_rule("Read"), cmd_allow_rule("Edit")],
            destination: PermissionUpdateDestination::Command,
        }])
        .await;

    let guard = store.read().await;
    assert_eq!(guard.len(), 2);
    assert_eq!(guard[0].value.tool_pattern, "Read");
    assert_eq!(guard[1].value.tool_pattern, "Edit");
}

#[tokio::test]
async fn empty_updates_is_no_op() {
    let store: Arc<RwLock<Vec<PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));
    let handle = EngineLiveRulesHandle::new(store.clone());
    handle.apply_updates(vec![]).await;
    assert!(store.read().await.is_empty());
}

#[tokio::test]
async fn non_command_destination_dropped() {
    let store: Arc<RwLock<Vec<PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));
    let handle = EngineLiveRulesHandle::new(store.clone());

    handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![cmd_allow_rule("Read")],
            destination: PermissionUpdateDestination::UserSettings,
        }])
        .await;

    // UserSettings persists via the settings store, not this handle.
    assert!(store.read().await.is_empty());
}

#[tokio::test]
async fn replace_remove_set_mode_dropped() {
    let store: Arc<RwLock<Vec<PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));
    let handle = EngineLiveRulesHandle::new(store.clone());

    handle
        .apply_updates(vec![
            PermissionUpdate::ReplaceRules {
                rules: vec![cmd_allow_rule("Read")],
                destination: PermissionUpdateDestination::Command,
            },
            PermissionUpdate::RemoveRules {
                rules: vec![cmd_allow_rule("Read")],
                destination: PermissionUpdateDestination::Command,
            },
            PermissionUpdate::SetMode {
                mode: coco_types::PermissionMode::AcceptEdits,
            },
        ])
        .await;

    // Skills only emit AddRules+Command; other variants are out of
    // scope for the live overlay.
    assert!(store.read().await.is_empty());
}

#[tokio::test]
async fn arc_sharing_propagates_writes() {
    // Verifies the Arc-sharing invariant the engine relies on: the
    // engine and the handle hold the same Arc, so a write via the
    // handle is observable through the engine's clone.
    let store: Arc<RwLock<Vec<PermissionRule>>> = Arc::new(RwLock::new(Vec::new()));
    let engine_view = store.clone();
    let handle = EngineLiveRulesHandle::new(store);

    handle
        .apply_updates(vec![PermissionUpdate::AddRules {
            rules: vec![cmd_allow_rule("Glob")],
            destination: PermissionUpdateDestination::Command,
        }])
        .await;

    assert_eq!(engine_view.read().await.len(), 1);
}
