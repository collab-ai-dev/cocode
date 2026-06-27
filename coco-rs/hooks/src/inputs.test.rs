//! Wire-format parity tests for hook input structs.
//!
//! These tests pin the JSON shape of every event-specific input so
//! a future refactor that drops or renames a field surfaces the
//! regression here, not in production.

use super::*;
use serde_json::Value;
use serde_json::json;

fn base() -> BaseHookInput {
    BaseHookInput {
        session_id: "sess".into(),
        cwd: "/cwd".into(),
        transcript_path: "/tmp/t.json".into(),
        permission_mode: Some("default".into()),
        agent_id: None,
        agent_type: None,
    }
}

/// `transcript_path` is required — emit it even when empty.
#[test]
fn base_hook_input_emits_transcript_path_when_empty() {
    let mut b = base();
    b.transcript_path = String::new();
    let v = serde_json::to_value(&b).unwrap();
    assert!(
        v.get("transcript_path").is_some(),
        "transcript_path must appear on the wire (required field)"
    );
    assert_eq!(v["transcript_path"], "");
}

/// `transcript_path` defaults to "" when missing on deserialize so
/// older fixtures keep parsing.
#[test]
fn base_hook_input_defaults_transcript_path() {
    let v: BaseHookInput = serde_json::from_value(json!({"session_id": "s", "cwd": "/c"})).unwrap();
    assert_eq!(v.transcript_path, "");
}

#[test]
fn pre_tool_use_carries_tool_use_id() {
    let input = PreToolUseInput {
        base: base(),
        tool_name: "Bash".into(),
        tool_input: json!({"command": "ls"}),
        tool_use_id: "tu-1".into(),
    };
    // Serialize through `HookInput` so the internally-tagged
    // `hook_event_name` discriminator is materialised on the wire —
    // the tag is supplied by the enum representation, not by an
    // inner-struct field.
    let v = serde_json::to_value(HookInput::PreToolUse(input)).unwrap();
    assert_eq!(v["hook_event_name"], "PreToolUse");
    assert_eq!(v["tool_name"], "Bash");
    assert_eq!(v["tool_use_id"], "tu-1");
    assert_eq!(v["tool_input"], json!({"command": "ls"}));
}

#[test]
fn post_tool_use_failure_emits_tool_use_id_and_is_interrupt() {
    let input = PostToolUseFailureInput {
        base: base(),
        tool_name: "Bash".into(),
        tool_input: json!({"command": "false"}),
        tool_use_id: "tu-9".into(),
        error: "exit 1".into(),
        is_interrupt: Some(true),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["tool_use_id"], "tu-9");
    assert_eq!(v["error"], "exit 1");
    assert_eq!(v["is_interrupt"], true);
    // Rust-only fields like the old `error_type` should NOT be on the wire.
    assert!(v.get("error_type").is_none());
}

#[test]
fn post_tool_use_failure_omits_is_interrupt_when_none() {
    let input = PostToolUseFailureInput {
        base: base(),
        tool_name: "Bash".into(),
        tool_input: json!({}),
        tool_use_id: "tu-9".into(),
        error: "boom".into(),
        is_interrupt: None,
    };
    let v = serde_json::to_value(&input).unwrap();
    assert!(v.get("is_interrupt").is_none());
}

#[test]
fn permission_denied_emits_tool_input_and_tool_use_id() {
    let input = PermissionDeniedInput {
        base: base(),
        tool_name: "Bash".into(),
        tool_input: json!({"command": "rm -rf /"}),
        tool_use_id: "tu-2".into(),
        reason: "auto-mode denied".into(),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["tool_input"], json!({"command": "rm -rf /"}));
    assert_eq!(v["tool_use_id"], "tu-2");
    assert_eq!(v["reason"], "auto-mode denied");
}

#[test]
fn permission_request_omits_tool_use_id_and_carries_suggestions() {
    let input = PermissionRequestInput {
        base: base(),
        tool_name: "Bash".into(),
        tool_input: json!({"command": "ls"}),
        permission_suggestions: Some(json!([{"behavior": "allow"}])),
    };
    let v = serde_json::to_value(&input).unwrap();
    // `tool_use_id` is not part of this event's schema.
    assert!(
        v.get("tool_use_id").is_none(),
        "PermissionRequest must not emit tool_use_id"
    );
    assert_eq!(v["permission_suggestions"], json!([{"behavior": "allow"}]));
}

#[test]
fn notification_carries_optional_title() {
    let input = NotificationInput {
        base: base(),
        message: "hi".into(),
        title: Some("Heads up".into()),
        notification_type: "permission_prompt".into(),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["title"], "Heads up");
    assert_eq!(v["message"], "hi");
    assert_eq!(v["notification_type"], "permission_prompt");
}

#[test]
fn stop_input_emits_stop_hook_active() {
    let input = StopInput {
        base: base(),
        stop_hook_active: true,
        last_assistant_message: Some("done".into()),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["stop_hook_active"], true);
    assert_eq!(v["last_assistant_message"], "done");
    // The old Rust-only `reason` field is gone.
    assert!(v.get("reason").is_none());
}

#[test]
fn subagent_stop_emits_stop_hook_active_and_required_transcript() {
    let input = SubagentStopInput {
        base: base(),
        stop_hook_active: false,
        agent_type: "Explore".into(),
        agent_id: "a-1".into(),
        agent_transcript_path: String::new(),
        last_assistant_message: None,
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["stop_hook_active"], false);
    assert_eq!(v["agent_transcript_path"], "");
    assert!(v.get("last_assistant_message").is_none());
}

#[test]
fn pre_compact_emits_null_for_missing_custom_instructions() {
    // `custom_instructions` MUST appear on the wire, with `null` when
    // absent. Hooks checking `input.custom_instructions === null` rely
    // on this shape.
    let input = PreCompactInput {
        base: base(),
        trigger: CompactTrigger::Auto,
        custom_instructions: None,
    };
    let v = serde_json::to_value(&input).unwrap();
    assert!(v.get("custom_instructions").is_some(), "must emit field");
    assert_eq!(v["custom_instructions"], Value::Null);
}

#[test]
fn pre_compact_emits_string_for_custom_instructions() {
    let input = PreCompactInput {
        base: base(),
        trigger: CompactTrigger::Manual,
        custom_instructions: Some("focus on TODOs".into()),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["custom_instructions"], "focus on TODOs");
}

#[test]
fn post_compact_requires_compact_summary() {
    let input = PostCompactInput {
        base: base(),
        trigger: CompactTrigger::Auto,
        compact_summary: "summary text".into(),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["compact_summary"], "summary text");
    // Reject the laxer combined struct: PostCompact must not have
    // a `custom_instructions` field on the wire.
    assert!(v.get("custom_instructions").is_none());
}

#[test]
fn elicitation_carries_optional_mode_url_id() {
    let input = ElicitationInput {
        base: base(),
        mcp_server_name: "github".into(),
        message: "Authorize?".into(),
        mode: Some(ElicitationMode::Form),
        url: Some("https://example.com/auth".into()),
        elicitation_id: Some("e-1".into()),
        requested_schema: Some(json!({"type": "object"})),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["mode"], "form");
    assert_eq!(v["url"], "https://example.com/auth");
    assert_eq!(v["elicitation_id"], "e-1");
    assert_eq!(v["requested_schema"], json!({"type": "object"}));
}

#[test]
fn instructions_loaded_carries_required_memory_type_and_optional_metadata() {
    let input = InstructionsLoadedInput {
        base: base(),
        file_path: "/p/CLAUDE.md".into(),
        memory_type: MemoryType::Project,
        load_reason: InstructionsLoadReason::SessionStart,
        globs: Some(vec!["**/*.rs".into()]),
        trigger_file_path: Some("/p/src/main.rs".into()),
        parent_file_path: None,
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["memory_type"], "Project");
    assert_eq!(v["load_reason"], "session_start");
    assert_eq!(v["globs"], json!(["**/*.rs"]));
    assert_eq!(v["trigger_file_path"], "/p/src/main.rs");
    assert!(v.get("parent_file_path").is_none());
}

#[test]
fn task_created_input_has_task_subject_not_task_type() {
    let input = TaskCreatedInput {
        base: base(),
        task_id: "t-1".into(),
        task_subject: "Refactor compaction".into(),
        task_description: Some("Split CompactHookInput".into()),
        teammate_name: None,
        team_name: None,
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["task_subject"], "Refactor compaction");
    assert_eq!(v["task_description"], "Split CompactHookInput");
    // The old shared TaskEventInput's `task_type` field is gone.
    assert!(v.get("task_type").is_none());
}

#[test]
fn teammate_idle_carries_teammate_and_team_names() {
    let input = TeammateIdleInput {
        base: base(),
        teammate_name: "Alice".into(),
        team_name: "frontend".into(),
    };
    let v = serde_json::to_value(&input).unwrap();
    assert_eq!(v["teammate_name"], "Alice");
    assert_eq!(v["team_name"], "frontend");
    // Old shape's task_id leak guard.
    assert!(v.get("task_id").is_none());
}

#[test]
fn enum_typed_fields_serialize_to_wire_strings() {
    // Spot-check every typed-enum field renders its expected wire value.
    assert_eq!(
        serde_json::to_value(SessionStartSource::Startup).unwrap(),
        "startup"
    );
    assert_eq!(
        serde_json::to_value(SessionStartSource::Compact).unwrap(),
        "compact"
    );
    assert_eq!(serde_json::to_value(SetupTrigger::Init).unwrap(), "init");
    assert_eq!(
        serde_json::to_value(SetupTrigger::Maintenance).unwrap(),
        "maintenance"
    );
    assert_eq!(
        serde_json::to_value(CompactTrigger::Manual).unwrap(),
        "manual"
    );
    assert_eq!(serde_json::to_value(CompactTrigger::Auto).unwrap(), "auto");
    assert_eq!(serde_json::to_value(ExitReason::Clear).unwrap(), "clear");
    assert_eq!(
        serde_json::to_value(ExitReason::PromptInputExit).unwrap(),
        "prompt_input_exit"
    );
    assert_eq!(
        serde_json::to_value(ExitReason::BypassPermissionsDisabled).unwrap(),
        "bypass_permissions_disabled"
    );
    assert_eq!(
        serde_json::to_value(FileChangeEvent::Change).unwrap(),
        "change"
    );
    assert_eq!(
        serde_json::to_value(FileChangeEvent::Unlink).unwrap(),
        "unlink"
    );
    assert_eq!(
        serde_json::to_value(ConfigChangeSource::PolicySettings).unwrap(),
        "policy_settings"
    );
    // PascalCase variant: memory_type uses 'User'/'Project'/'Local'/'Managed'
    // — Rust default serde rename keeps PascalCase.
    assert_eq!(
        serde_json::to_value(MemoryType::Project).unwrap(),
        "Project"
    );
    assert_eq!(
        serde_json::to_value(MemoryType::Managed).unwrap(),
        "Managed"
    );
    assert_eq!(
        serde_json::to_value(InstructionsLoadReason::SessionStart).unwrap(),
        "session_start"
    );
    assert_eq!(
        serde_json::to_value(InstructionsLoadReason::PathGlobMatch).unwrap(),
        "path_glob_match"
    );
    assert_eq!(serde_json::to_value(ElicitationMode::Form).unwrap(), "form");
    assert_eq!(
        serde_json::to_value(ElicitationAction::Accept).unwrap(),
        "accept"
    );
}

#[test]
fn enum_wire_strs_match_serde_output() {
    // The manual `as_wire_str()` must produce the exact same string
    // that serde serializes — otherwise hook matchers (which key off
    // `as_wire_str`) would diverge from the JSON the hook reads.
    fn assert_match<T: serde::Serialize>(value: &T, wire: &str) {
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::json!(wire)
        );
    }
    assert_match(
        &SessionStartSource::Resume,
        SessionStartSource::Resume.as_wire_str(),
    );
    assert_match(
        &SetupTrigger::Maintenance,
        SetupTrigger::Maintenance.as_wire_str(),
    );
    assert_match(&CompactTrigger::Auto, CompactTrigger::Auto.as_wire_str());
    assert_match(&ExitReason::Other, ExitReason::Other.as_wire_str());
    assert_match(&FileChangeEvent::Add, FileChangeEvent::Add.as_wire_str());
    assert_match(
        &ConfigChangeSource::UserSettings,
        ConfigChangeSource::UserSettings.as_wire_str(),
    );
    assert_match(&MemoryType::Local, MemoryType::Local.as_wire_str());
    assert_match(
        &InstructionsLoadReason::Include,
        InstructionsLoadReason::Include.as_wire_str(),
    );
    assert_match(&ElicitationMode::Url, ElicitationMode::Url.as_wire_str());
    assert_match(
        &ElicitationAction::Decline,
        super::elicitation_action_wire_str(ElicitationAction::Decline),
    );
}

// ---------------------------------------------------------------------------
// HookInput envelope round-trip
// ---------------------------------------------------------------------------

/// Every variant must serialize through `HookInput` with the
/// `hook_event_name` tag at the top level (supplied by serde's
/// internally-tagged representation) and deserialize back into the
/// matching variant. Pins both the wire shape and the
/// `flatten BaseHookInput + tagged-enum dispatch` interaction so a
/// future serde upgrade or schemars change can't silently regress.
#[test]
fn hook_input_round_trip_carries_tag_for_every_variant() {
    let cases: Vec<(HookInput, HookEventType)> = vec![
        (
            HookInput::PreToolUse(PreToolUseInput {
                base: base(),
                tool_name: "Bash".into(),
                tool_input: json!({"command": "ls"}),
                tool_use_id: "tu-1".into(),
            }),
            HookEventType::PreToolUse,
        ),
        (
            HookInput::PostToolUse(PostToolUseInput {
                base: base(),
                tool_name: "Bash".into(),
                tool_input: json!({"command": "ls"}),
                tool_response: json!({"stdout": ""}),
                tool_use_id: "tu-1".into(),
            }),
            HookEventType::PostToolUse,
        ),
        (
            HookInput::PostToolUseFailure(PostToolUseFailureInput {
                base: base(),
                tool_name: "Bash".into(),
                tool_input: json!({}),
                tool_use_id: "tu-1".into(),
                error: "boom".into(),
                is_interrupt: None,
            }),
            HookEventType::PostToolUseFailure,
        ),
        (
            HookInput::SessionStart(SessionStartInput {
                base: base(),
                source: SessionStartSource::Startup,
                agent_type: None,
                model: None,
            }),
            HookEventType::SessionStart,
        ),
        (
            HookInput::SessionEnd(SessionEndInput {
                base: base(),
                reason: ExitReason::Clear,
            }),
            HookEventType::SessionEnd,
        ),
        (
            HookInput::Setup(SetupInput {
                base: base(),
                trigger: SetupTrigger::Init,
            }),
            HookEventType::Setup,
        ),
        (
            HookInput::Stop(StopInput {
                base: base(),
                stop_hook_active: false,
                last_assistant_message: None,
            }),
            HookEventType::Stop,
        ),
        (
            HookInput::StopFailure(StopFailureInput {
                base: base(),
                error: "boom".into(),
                error_details: None,
                last_assistant_message: None,
            }),
            HookEventType::StopFailure,
        ),
        (
            HookInput::PreCompact(PreCompactInput {
                base: base(),
                trigger: CompactTrigger::Auto,
                custom_instructions: None,
            }),
            HookEventType::PreCompact,
        ),
        (
            HookInput::PostCompact(PostCompactInput {
                base: base(),
                trigger: CompactTrigger::Auto,
                compact_summary: "s".into(),
            }),
            HookEventType::PostCompact,
        ),
        (
            HookInput::SubagentStart(SubagentStartInput {
                base: base(),
                agent_type: "Explore".into(),
                agent_id: "a-1".into(),
            }),
            HookEventType::SubagentStart,
        ),
        (
            HookInput::SubagentStop(SubagentStopInput {
                base: base(),
                stop_hook_active: false,
                agent_type: "Explore".into(),
                agent_id: "a-1".into(),
                agent_transcript_path: String::new(),
                last_assistant_message: None,
            }),
            HookEventType::SubagentStop,
        ),
        (
            HookInput::UserPromptSubmit(UserPromptSubmitInput {
                base: base(),
                prompt: "hi".into(),
            }),
            HookEventType::UserPromptSubmit,
        ),
        (
            HookInput::PermissionRequest(PermissionRequestInput {
                base: base(),
                tool_name: "Bash".into(),
                tool_input: json!({}),
                permission_suggestions: None,
            }),
            HookEventType::PermissionRequest,
        ),
        (
            HookInput::PermissionDenied(PermissionDeniedInput {
                base: base(),
                tool_name: "Bash".into(),
                tool_input: json!({}),
                tool_use_id: "tu-1".into(),
                reason: "denied".into(),
            }),
            HookEventType::PermissionDenied,
        ),
        (
            HookInput::Notification(NotificationInput {
                base: base(),
                message: "hi".into(),
                title: None,
                notification_type: "permission_prompt".into(),
            }),
            HookEventType::Notification,
        ),
        (
            HookInput::Elicitation(ElicitationInput {
                base: base(),
                mcp_server_name: "s".into(),
                message: "m".into(),
                mode: None,
                url: None,
                elicitation_id: None,
                requested_schema: None,
            }),
            HookEventType::Elicitation,
        ),
        (
            HookInput::ElicitationResult(ElicitationResultInput {
                base: base(),
                mcp_server_name: "s".into(),
                elicitation_id: None,
                mode: None,
                action: ElicitationAction::Accept,
                content: None,
            }),
            HookEventType::ElicitationResult,
        ),
        (
            HookInput::FileChanged(FileChangedInput {
                base: base(),
                file_path: "/f".into(),
                event: FileChangeEvent::Change,
            }),
            HookEventType::FileChanged,
        ),
        (
            HookInput::ConfigChange(ConfigChangeInput {
                base: base(),
                source: ConfigChangeSource::UserSettings,
                file_path: None,
            }),
            HookEventType::ConfigChange,
        ),
        (
            HookInput::InstructionsLoaded(InstructionsLoadedInput {
                base: base(),
                file_path: "/CLAUDE.md".into(),
                memory_type: MemoryType::Project,
                load_reason: InstructionsLoadReason::SessionStart,
                globs: None,
                trigger_file_path: None,
                parent_file_path: None,
            }),
            HookEventType::InstructionsLoaded,
        ),
        (
            HookInput::CwdChanged(CwdChangedInput {
                base: base(),
                old_cwd: "/a".into(),
                new_cwd: "/b".into(),
            }),
            HookEventType::CwdChanged,
        ),
        (
            HookInput::WorktreeCreate(WorktreeCreateInput {
                base: base(),
                name: "wt".into(),
            }),
            HookEventType::WorktreeCreate,
        ),
        (
            HookInput::WorktreeRemove(WorktreeRemoveInput {
                base: base(),
                worktree_path: "/wt".into(),
            }),
            HookEventType::WorktreeRemove,
        ),
        (
            HookInput::TaskCreated(TaskCreatedInput {
                base: base(),
                task_id: "t-1".into(),
                task_subject: "s".into(),
                task_description: None,
                teammate_name: None,
                team_name: None,
            }),
            HookEventType::TaskCreated,
        ),
        (
            HookInput::TaskCompleted(TaskCompletedInput {
                base: base(),
                task_id: "t-1".into(),
                task_subject: "s".into(),
                task_description: None,
                teammate_name: None,
                team_name: None,
            }),
            HookEventType::TaskCompleted,
        ),
        (
            HookInput::TeammateIdle(TeammateIdleInput {
                base: base(),
                teammate_name: "Alice".into(),
                team_name: "frontend".into(),
            }),
            HookEventType::TeammateIdle,
        ),
    ];

    assert_eq!(cases.len(), 27, "must cover every HookEventType variant");

    for (input, expected_event) in cases {
        let wire = serde_json::to_value(&input).unwrap();
        assert_eq!(
            wire["hook_event_name"],
            expected_event.as_str(),
            "tag on wire for {expected_event:?}"
        );
        // BaseHookInput fields must appear at the top level, not nested
        // — flatten + internal tag must continue to merge into one
        // object.
        assert_eq!(wire["session_id"], "sess");
        assert_eq!(wire["cwd"], "/cwd");

        // Round-trip back through HookInput: variant + event must
        // survive the deserialize → re-serialize cycle.
        let parsed: HookInput = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("round-trip parse failed for {expected_event:?}: {e}"));
        assert_eq!(parsed.event(), expected_event);
        let reserialised = serde_json::to_value(&parsed).unwrap();
        assert_eq!(
            reserialised, wire,
            "stable wire format for {expected_event:?}"
        );
    }
}
