use super::*;
use pretty_assertions::assert_eq;

#[test]
fn apply_patch_preview_serializes_snake_case_rows() {
    let preview = ApplyPatchPreview {
        rows: vec![
            ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Update,
                target: "old.rs -> new.rs".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Added,
                content: "new line".to_string(),
            },
            ApplyPatchPreviewRow::Raw {
                content: "*** Update File: broken.rs".to_string(),
            },
            ApplyPatchPreviewRow::Omitted { rows: 7 },
        ],
    };

    let value = serde_json::to_value(&preview).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "rows": [
                {
                    "kind": "header",
                    "action": "update",
                    "target": "old.rs -> new.rs"
                },
                {
                    "kind": "line",
                    "sign": "added",
                    "content": "new line"
                },
                {
                    "kind": "raw",
                    "content": "*** Update File: broken.rs"
                },
                {
                    "kind": "omitted",
                    "rows": 7
                }
            ]
        })
    );
}

#[test]
fn tool_display_data_serializes_apply_patch_preview() {
    let data = ToolDisplayData::ApplyPatchPreview(ApplyPatchPreview {
        rows: vec![ApplyPatchPreviewRow::Omitted { rows: 3 }],
    });

    let value = serde_json::to_value(&data).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "kind": "apply_patch_preview",
            "data": {
                "rows": [
                    {
                        "kind": "omitted",
                        "rows": 3
                    }
                ]
            }
        })
    );
    assert_eq!(
        serde_json::from_value::<ToolDisplayData>(value).unwrap(),
        data
    );
}

#[test]
fn tool_display_data_serializes_exit_plan_mode_result() {
    let data = ToolDisplayData::ExitPlanModeResult(ExitPlanModeResult {
        outcome: ExitPlanModeOutcome::ImplementationPlan,
        plan: "# Plan\n- step".to_string(),
        file_path: Some("/tmp/session-plan.md".to_string()),
        awaiting_leader_approval: false,
        is_agent: false,
        plan_was_edited: true,
    });

    let value = serde_json::to_value(&data).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "kind": "exit_plan_mode_result",
                "data": {
                "outcome": "implementation_plan",
                "plan": "# Plan\n- step",
                "filePath": "/tmp/session-plan.md",
                "awaitingLeaderApproval": false,
                "isAgent": false,
                "planWasEdited": true
            }
        })
    );
    assert_eq!(
        serde_json::from_value::<ToolDisplayData>(value).unwrap(),
        data
    );
}

#[test]
fn exit_plan_mode_result_distinguishes_no_plan_notice() {
    let no_plan = ExitPlanModeResult {
        outcome: ExitPlanModeOutcome::NoImplementationPlan,
        plan: "User asked for a read-only explanation.".to_string(),
        file_path: Some("/tmp/session-plan.md".to_string()),
        awaiting_leader_approval: false,
        is_agent: false,
        plan_was_edited: false,
    };
    let plan = ExitPlanModeResult {
        outcome: ExitPlanModeOutcome::ImplementationPlan,
        plan: "# Plan\n- Update code".to_string(),
        file_path: Some("/tmp/session-plan.md".to_string()),
        awaiting_leader_approval: false,
        is_agent: false,
        plan_was_edited: false,
    };

    assert!(!no_plan.has_implementation_plan());
    assert!(plan.has_implementation_plan());
}

#[test]
fn apply_patch_preview_rejects_negative_omitted_rows() {
    let err = serde_json::from_value::<ApplyPatchPreview>(serde_json::json!({
        "rows": [
            {
                "kind": "omitted",
                "rows": -1
            }
        ]
    }))
    .unwrap_err();

    assert!(
        err.to_string().contains("non-negative"),
        "unexpected error: {err}"
    );
}

#[test]
fn apply_patch_preview_helpers_expose_display_tokens() {
    assert_eq!(ApplyPatchPreviewAction::Add.as_str(), "add");
    assert_eq!(ApplyPatchPreviewAction::Delete.as_str(), "delete");
    assert_eq!(ApplyPatchPreviewAction::Update.as_str(), "update");
    assert_eq!(ApplyPatchPreviewSign::Added.as_char(), '+');
    assert_eq!(ApplyPatchPreviewSign::Removed.as_char(), '-');
    assert_eq!(ApplyPatchPreviewSign::Context.as_char(), ' ');
}
