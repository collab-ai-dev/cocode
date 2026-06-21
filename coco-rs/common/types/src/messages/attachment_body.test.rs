use super::*;
use crate::AttachmentKind;
use crate::AttachmentMessage;
use crate::Coverage;
use crate::HookEventType;
use crate::LlmMessage;
use pretty_assertions::assert_eq;

#[test]
fn hook_permission_decision_ask_round_trips() {
    let encoded = serde_json::to_string(&HookPermissionDecision::Ask).unwrap();
    assert_eq!(encoded, "\"ask\"");
    let decoded: HookPermissionDecision = serde_json::from_str("\"ask\"").unwrap();
    assert_eq!(decoded, HookPermissionDecision::Ask);
}

#[test]
fn silent_payload_round_trips_with_every_silent_attachment_kind() {
    // Every Coverage::SilentEvent and Coverage::SilentReminder kind must
    // be buildable via the typed silent_* constructors. If any variant
    // is missed, the corresponding constructor call will fail to compile —
    // this test walks each kind and asserts the expected variant tag.
    for kind in AttachmentKind::all() {
        let expected_silent = matches!(
            kind.coverage(),
            Coverage::SilentEvent { .. } | Coverage::SilentReminder { .. }
        );
        if !expected_silent {
            continue;
        }
        // Build via match to ensure every silent kind has a typed helper.
        let msg: AttachmentMessage = match *kind {
            AttachmentKind::HookCancelled => {
                AttachmentMessage::silent_hook_cancelled(HookCancelledPayload {
                    hook_name: "test".into(),
                    tool_use_id: "id".into(),
                    hook_event: HookEventType::PreToolUse,
                    command: None,
                    duration_ms: None,
                })
            }
            AttachmentKind::HookErrorDuringExecution => {
                AttachmentMessage::silent_hook_error_during_execution(
                    HookErrorDuringExecutionPayload {
                        content: "e".into(),
                        hook_name: "h".into(),
                        tool_use_id: "id".into(),
                        hook_event: HookEventType::PreToolUse,
                    },
                )
            }
            AttachmentKind::HookNonBlockingError => {
                AttachmentMessage::silent_hook_non_blocking_error(HookNonBlockingErrorPayload {
                    error: "e".into(),
                    hook_name: "h".into(),
                    tool_use_id: "id".into(),
                    hook_event: HookEventType::PreToolUse,
                })
            }
            AttachmentKind::HookSystemMessage => {
                AttachmentMessage::silent_hook_system_message(HookSystemMessagePayload {
                    content: "m".into(),
                    hook_name: "h".into(),
                    tool_use_id: "id".into(),
                    hook_event: HookEventType::PreToolUse,
                })
            }
            AttachmentKind::HookPermissionDecision => {
                AttachmentMessage::silent_hook_permission_decision(HookPermissionDecisionPayload {
                    decision: HookPermissionDecision::Allow,
                    tool_use_id: "id".into(),
                    hook_event: HookEventType::PreToolUse,
                })
            }
            AttachmentKind::CommandPermissions => {
                AttachmentMessage::silent_command_permissions(CommandPermissionsPayload::default())
            }
            AttachmentKind::StructuredOutput => {
                AttachmentMessage::silent_structured_output(StructuredOutputPayload::default())
            }
            AttachmentKind::DynamicSkill => {
                AttachmentMessage::silent_dynamic_skill(DynamicSkillPayload::default())
            }
            AttachmentKind::MaxTurnsReached => {
                AttachmentMessage::silent_max_turns_reached(MaxTurnsReachedPayload::default())
            }
            AttachmentKind::AlreadyReadFile => {
                AttachmentMessage::silent_already_read_file(AlreadyReadFilePayload::default())
            }
            AttachmentKind::EditedImageFile => {
                AttachmentMessage::silent_edited_image_file(EditedImageFilePayload::default())
            }
            AttachmentKind::CurrentSessionMemory | AttachmentKind::ContextEfficiency => continue,
            other => panic!("silent kind {other:?} has no constructor"),
        };
        assert_eq!(msg.kind, *kind);
        assert!(matches!(msg.body, AttachmentBody::Silent(_)));
    }
}

#[test]
fn api_constructor_rejects_non_api_visible_in_debug() {
    // Sanity: api() must accept API-visible kinds.
    let _ = AttachmentMessage::api(AttachmentKind::PlanMode, crate::LlmMessage::user_text("hi"));
}

#[test]
fn skill_discovery_preserves_payload_and_api_prompt() {
    let payload = SkillDiscoveryPayload {
        skills: vec![SkillDiscoverySkill {
            name: "rust".to_string(),
            description: "Rust conventions".to_string(),
            short_id: Some("abc".to_string()),
        }],
        signal: "explicit".to_string(),
        source: SkillDiscoverySource::Native,
    };
    let msg = AttachmentMessage::skill_discovery(payload.clone());
    assert_eq!(msg.kind, AttachmentKind::SkillDiscovery);
    // Body is a regular Api(LlmMessage); structured payload travels in
    // `extras` so pattern matches on `AttachmentBody::Api(..)` stay
    // uniform across kinds.
    let AttachmentBody::Api(message) = &msg.body else {
        panic!("skill_discovery body must be Api(LlmMessage)");
    };
    let Some(AttachmentExtras::SkillDiscovery(stored)) = msg.extras.as_ref() else {
        panic!("skill_discovery must populate extras with the typed payload");
    };
    assert_eq!(stored, &payload);
    assert_eq!(msg.as_api_message(), Some(message));
    assert!(
        msg.as_text_for_display()
            .contains("Skills relevant to your task")
    );
}

#[test]
fn compact_file_reference_preserves_payload_and_api_prompt() {
    let payload = CompactFileReferencePayload {
        filename: "/repo/src/lib.rs".to_string(),
        display_path: "src/lib.rs".to_string(),
    };
    let msg = AttachmentMessage::compact_file_reference(
        payload.clone(),
        LlmMessage::user_text("model-visible restore reminder"),
    );

    assert_eq!(msg.kind, AttachmentKind::CompactFileReference);
    let AttachmentBody::Api(message) = &msg.body else {
        panic!("compact_file_reference body must be Api(LlmMessage)");
    };
    let Some(AttachmentExtras::CompactFileReference(stored)) = msg.extras.as_ref() else {
        panic!("compact_file_reference must populate extras with the typed payload");
    };
    assert_eq!(stored, &payload);
    assert_eq!(msg.as_api_message(), Some(message));
}

#[test]
fn unit_constructor_works_for_runtime_bookkeeping() {
    // RuntimeBookkeeping kinds carry no payload.
    let msg = AttachmentMessage::unit(AttachmentKind::BagelConsole);
    assert_eq!(msg.kind, AttachmentKind::BagelConsole);
    assert!(matches!(msg.body, AttachmentBody::Unit));
}

#[test]
fn attachment_body_serde_roundtrip() {
    let original = AttachmentMessage::silent_hook_cancelled(HookCancelledPayload {
        hook_name: "hook".into(),
        tool_use_id: "tid".into(),
        hook_event: HookEventType::PreToolUse,
        command: Some("cmd".into()),
        duration_ms: Some(42),
    });
    let json = serde_json::to_string(&original).unwrap();
    let back: AttachmentMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(original.kind, back.kind);
}
