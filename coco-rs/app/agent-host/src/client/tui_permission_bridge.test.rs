use super::*;
use coco_types::CoreEvent;

fn dummy_request(id: &str) -> ToolPermissionRequest {
    ToolPermissionRequest {
        id: id.into(),
        tool_use_id: format!("use-{id}"),
        agent_id: "leader".into(),
        tool_name: "Bash".into(),
        description: "ls".into(),
        input: serde_json::json!({"command": "ls"}),
        cwd: None,
        suggestions: vec![],
        choices: None,
        detail: None,
        worker_badge: None,
    }
}

fn ask_user_question_request(id: &str) -> ToolPermissionRequest {
    ToolPermissionRequest {
        id: id.into(),
        tool_use_id: format!("use-{id}"),
        agent_id: "leader".into(),
        tool_name: coco_types::ToolName::AskUserQuestion.as_str().into(),
        description: "Answer questions?".into(),
        input: serde_json::json!({
            "questions": [{
                "question": "Which approach?",
                "header": "Approach",
                "options": [
                    {"label": "A", "description": "Use A"},
                    {"label": "B", "description": "Use B"}
                ],
                "multiSelect": false
            }]
        }),
        cwd: None,
        suggestions: vec![],
        choices: None,
        detail: None,
        worker_badge: None,
    }
}

fn exit_plan_mode_request(id: &str) -> ToolPermissionRequest {
    ToolPermissionRequest {
        id: id.into(),
        tool_use_id: format!("use-{id}"),
        agent_id: "leader".into(),
        tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
        description: "Exit plan mode?".into(),
        input: serde_json::json!({
            "outcome": "implementation_plan",
            "plan": "# Plan\n\n- Change the thing",
            "planFilePath": "/tmp/plan.md",
            "allowedPrompts": [{"tool": "Bash", "prompt": "cargo test"}]
        }),
        cwd: None,
        suggestions: vec![],
        choices: Some(vec![
            coco_types::PermissionAskChoice {
                value: "yes-accept-edits-keep-context".into(),
                label: "Yes, auto-accept edits".into(),
                description: None,
            },
            coco_types::PermissionAskChoice {
                value: "yes-default-keep-context".into(),
                label: "Yes, manually approve edits".into(),
                description: None,
            },
            coco_types::PermissionAskChoice {
                value: "no".into(),
                label: "No, keep planning".into(),
                description: None,
            },
        ]),
        detail: Some(coco_types::PermissionRequestDetail::ExitPlanMode {
            outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
            plan: Some("# Plan\n\n- Change the thing".into()),
            plan_file_path: Some("/tmp/plan.md".into()),
            allowed_prompts: vec![coco_types::ExitPlanModeAllowedPrompt {
                tool: "Bash".into(),
                prompt: "cargo test".into(),
            }],
        }),
        worker_badge: None,
    }
}

#[tokio::test]
async fn approve_flow_sends_approved_decision() {
    let pending = new_pending_map();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(8);
    let bridge = TuiPermissionBridge::new(tx, pending.clone());

    let request_handle =
        tokio::spawn(async move { bridge.request_permission(dummy_request("r1")).await });

    // Bridge should emit ApprovalRequired before awaiting.
    let event = rx.recv().await.expect("bridge emits an event");
    match event {
        CoreEvent::Tui(TuiOnlyEvent::ApprovalRequired {
            request_id,
            description,
            display_input,
            show_always_allow,
            ..
        }) => {
            assert_eq!(request_id, "r1");
            assert_eq!(description, "ls");
            assert_eq!(
                display_input,
                coco_types::PermissionDisplayInput::Command("ls".into())
            );
            assert!(show_always_allow);
        }
        other => panic!("expected Tui(ApprovalRequired); got {other:?}"),
    }

    // Simulate user approval.
    let resolved = resolve_pending(&pending, "r1", true, None, Vec::new(), None, None, None).await;
    assert!(resolved);

    let resolution = request_handle.await.unwrap().unwrap();
    assert_eq!(resolution.decision, ToolPermissionDecision::Approved);
}

#[tokio::test]
async fn reject_flow_propagates_feedback() {
    let pending = new_pending_map();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(8);
    let bridge = TuiPermissionBridge::new(tx, pending.clone());

    let handle = tokio::spawn(async move { bridge.request_permission(dummy_request("r2")).await });
    let _ = rx.recv().await;

    let resolved = resolve_pending(
        &pending,
        "r2",
        false,
        Some("not safe".into()),
        Vec::new(),
        None,
        None,
        None,
    )
    .await;
    assert!(resolved);

    let resolution = handle.await.unwrap().unwrap();
    assert_eq!(resolution.decision, ToolPermissionDecision::Rejected);
    assert_eq!(resolution.feedback.as_deref(), Some("not safe"));
}

#[tokio::test]
async fn ask_user_question_emits_question_asked_event() {
    let pending = new_pending_map();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(8);
    let bridge = TuiPermissionBridge::new(tx, pending.clone());

    let handle = tokio::spawn(async move {
        bridge
            .request_permission(ask_user_question_request("q1"))
            .await
    });

    let event = rx.recv().await.expect("bridge emits an event");
    let input = match event {
        CoreEvent::Tui(TuiOnlyEvent::QuestionAsked { request_id, input }) => {
            assert_eq!(request_id, "q1");
            input
        }
        other => panic!("expected Tui(QuestionAsked); got {other:?}"),
    };
    assert!(input["questions"].is_array());

    let updated_input = serde_json::json!({
        "questions": input["questions"].clone(),
        "answers": {"Which approach?": "A"}
    });
    let resolved = resolve_pending(
        &pending,
        "q1",
        true,
        None,
        Vec::new(),
        Some(updated_input.clone()),
        None,
        None,
    )
    .await;
    assert!(resolved);

    let resolution = handle.await.unwrap().unwrap();
    assert_eq!(resolution.decision, ToolPermissionDecision::Approved);
    assert_eq!(resolution.updated_input, Some(updated_input));
}

#[tokio::test]
async fn exit_plan_mode_emits_dedicated_choices_without_always_allow() {
    let pending = new_pending_map();
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(8);
    let bridge = TuiPermissionBridge::new(tx, pending.clone());

    let handle = tokio::spawn(async move {
        bridge
            .request_permission(exit_plan_mode_request("plan1"))
            .await
    });

    let event = rx.recv().await.expect("bridge emits an event");
    match event {
        CoreEvent::Tui(TuiOnlyEvent::ApprovalRequired {
            request_id,
            tool_name,
            display_input,
            show_always_allow,
            choices,
            original_input,
            ..
        }) => {
            assert_eq!(request_id, "plan1");
            assert_eq!(tool_name, coco_types::ToolName::ExitPlanMode.as_str());
            assert_eq!(display_input, coco_types::PermissionDisplayInput::Empty);
            assert!(!show_always_allow);
            let values: Vec<&str> = choices
                .as_ref()
                .expect("ExitPlanMode choices")
                .iter()
                .map(|c| c.value.as_str())
                .collect();
            assert_eq!(
                values,
                vec![
                    "yes-accept-edits-keep-context",
                    "yes-default-keep-context",
                    "no"
                ]
            );
            assert_eq!(
                original_input
                    .as_ref()
                    .and_then(|i| i.get("plan"))
                    .and_then(serde_json::Value::as_str),
                Some("# Plan\n\n- Change the thing")
            );
        }
        other => panic!("expected Tui(ApprovalRequired); got {other:?}"),
    }

    let resolved =
        resolve_pending(&pending, "plan1", false, None, Vec::new(), None, None, None).await;
    assert!(resolved);
    let resolution = handle.await.unwrap().unwrap();
    assert_eq!(resolution.decision, ToolPermissionDecision::Rejected);
}

#[tokio::test]
async fn unknown_request_id_returns_false() {
    let pending = new_pending_map();
    let resolved =
        resolve_pending(&pending, "ghost", true, None, Vec::new(), None, None, None).await;
    assert!(!resolved);
}

#[tokio::test]
async fn take_pending_removes_entry_before_resolution() {
    let pending = new_pending_map();
    let (tx, rx) = oneshot::channel();
    pending.write().await.insert(
        "r4".into(),
        PendingApprovalEntry {
            sender: tx,
            _guard: None,
        },
    );

    let entry = take_pending(&pending, "r4")
        .await
        .expect("pending entry exists");
    assert!(take_pending(&pending, "r4").await.is_none());
    assert!(send_resolution(
        entry,
        true,
        None,
        Vec::new(),
        None,
        None,
        None
    ));

    let resolution = rx.await.expect("resolution sent");
    assert_eq!(resolution.decision, ToolPermissionDecision::Approved);
}

#[test]
fn settings_allow_always_allow_options_defaults_to_true() {
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings::default(),
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };

    assert!(settings_allow_always_allow_options(&settings));
}

#[test]
fn settings_allow_always_allow_options_respects_managed_policy_camel_case() {
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings::default(),
        per_source: std::collections::HashMap::from([(
            coco_config::SettingSource::Policy,
            serde_json::json!({
                "permissions": {
                    "allowManagedPermissionRulesOnly": true
                }
            }),
        )]),
        source_paths: std::collections::HashMap::new(),
    };

    assert!(!settings_allow_always_allow_options(&settings));
}

#[test]
fn settings_allow_always_allow_options_respects_managed_policy_snake_case() {
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings::default(),
        per_source: std::collections::HashMap::from([(
            coco_config::SettingSource::Policy,
            serde_json::json!({
                "permissions": {
                    "allow_managed_permission_rules_only": true
                }
            }),
        )]),
        source_paths: std::collections::HashMap::new(),
    };

    assert!(!settings_allow_always_allow_options(&settings));
}

#[tokio::test]
async fn channel_close_returns_error() {
    let pending = new_pending_map();
    let (tx, _rx) = mpsc::channel::<CoreEvent>(8);
    drop(_rx); // close the channel before the bridge sends

    let bridge = TuiPermissionBridge::new(tx, pending.clone());
    let result = bridge.request_permission(dummy_request("r3")).await;
    assert!(
        result.is_err(),
        "channel closed → request_permission errors"
    );
    // Pending map should not retain the entry.
    assert!(pending.read().await.is_empty());
}

#[tokio::test]
async fn test_explain_risk_none_without_runtime() {
    // No SessionRuntime bound -> the Weak upgrade fails -> graceful None
    // (the explainer panel simply shows no risk badge).
    let pending = new_pending_map();
    let (tx, _rx) = mpsc::channel::<CoreEvent>(8);
    let bridge = TuiPermissionBridge::new(tx, pending);
    let input = serde_json::json!({"command": "rm -rf /"});
    let params = coco_permissions::ExplainerParams {
        tool_name: "Bash",
        tool_input: &input,
        tool_description: None,
        messages: None,
    };
    assert!(bridge.explain_risk(params).await.is_none());
}
