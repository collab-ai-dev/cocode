use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashSet;

/// Size guard — if TS adds an `Attachment.type`, this test fails and the
/// author has to extend `AttachmentKind` + `coverage_of` before
/// continuing. Update both the constant and the TS `Attachment` union
/// size in the README.
#[test]
fn attachment_kind_all_has_62_variants() {
    assert_eq!(
        AttachmentKind::all().len(),
        62,
        "60 TS Attachment union members + coco-rs-synthetic user_context and slash_command_metadata"
    );
}

/// `as_str` must be unique across every variant (otherwise serde
/// round-trips collide). Cheap insurance against a cut-and-paste typo
/// in the big match.
#[test]
fn as_str_values_are_unique() {
    let mut set = HashSet::new();
    for k in AttachmentKind::all() {
        assert!(
            set.insert(k.as_str()),
            "duplicate wire string: {}",
            k.as_str()
        );
    }
}

/// Coverage must be assigned for every variant. The match in
/// `coverage_of` is already exhaustive — this test just exercises it so
/// any future `unreachable!()`-style escape hatch would be caught.
#[test]
fn every_variant_has_coverage() {
    for k in AttachmentKind::all() {
        // No assertion — the call itself is the test. If coverage_of
        // ever grows a panic path for unmapped variants, this test
        // would blow up.
        let _ = k.coverage();
    }
}

/// Distribution guard — if someone reclassifies a reminder into
/// SilentEvent (or vice versa) without updating this tally, the counts
/// shift and this test fails, which is a prompt to double-check the
/// intent.
#[test]
fn coverage_distribution_matches_readme_snapshot() {
    let mut reminder = 0;
    let mut silent_reminder = 0;
    let mut outside = 0;
    let mut silent_event = 0;
    let mut feature_gated = 0;
    let mut runtime = 0;
    for k in AttachmentKind::all() {
        match k.coverage() {
            Coverage::Reminder { .. } => reminder += 1,
            Coverage::SilentReminder { .. } => silent_reminder += 1,
            Coverage::OutsideReminder { .. } => outside += 1,
            Coverage::SilentEvent { .. } => silent_event += 1,
            Coverage::FeatureGated { .. } => feature_gated += 1,
            Coverage::RuntimeBookkeeping { .. } => runtime += 1,
        }
    }
    // Snapshot after reminder split (May 2026): skill_discovery
    // is the only audit-add kind owned by the model-visible reminder path;
    // API-hidden audit-add kinds are typed silent events or inactive runtime
    // bookkeeping.
    assert_eq!(
        reminder, 40,
        "in-crate reminders (incl. synthetic user_context)"
    );
    assert_eq!(silent_reminder, 2, "in-crate silent reminders");
    assert_eq!(outside, 7, "owned by sister crates");
    assert_eq!(silent_event, 9, "typed API-hidden events");
    assert_eq!(feature_gated, 1, "HISTORY_SNIP intentionally unported");
    assert_eq!(runtime, 3, "inactive runtime bookkeeping");
    assert_eq!(
        reminder + silent_reminder + outside + silent_event + feature_gated + runtime,
        62,
        "total must match union size + synthetic user_context + slash_command_metadata"
    );
}

#[test]
fn serde_roundtrips_wire_string() {
    let json = serde_json::to_string(&AttachmentKind::PlanMode).unwrap();
    assert_eq!(json, "\"plan_mode\"");
    let back: AttachmentKind = serde_json::from_str("\"already_read_file\"").unwrap();
    assert_eq!(back, AttachmentKind::AlreadyReadFile);
}

#[test]
fn display_matches_as_str() {
    assert_eq!(
        format!("{}", AttachmentKind::HookCancelled),
        "hook_cancelled"
    );
}

#[test]
fn skill_listing_is_api_visible_but_transcript_hidden() {
    assert!(AttachmentKind::SkillListing.is_api_visible());
    assert!(!AttachmentKind::SkillListing.renders_in_transcript());
}

/// TS parity guard: every kind listed in
/// `components/messages/nullRenderingAttachments.ts:14-49` (TS
/// `NULL_RENDERING_TYPES`) must return `renders_in_transcript() == false`
/// in coco-rs. If someone adds a kind to the TS list upstream and forgets
/// the Rust predicate, this test blows up.
#[test]
fn renders_in_transcript_matches_ts_null_rendering_list_exact() {
    // Verbatim copy of TS NULL_RENDERING_TYPES snapshot. Keep in sync.
    let ts_null_rendering = [
        AttachmentKind::HookSuccess,
        AttachmentKind::HookAdditionalContext,
        AttachmentKind::HookCancelled,
        AttachmentKind::CommandPermissions,
        AttachmentKind::AgentMention,
        AttachmentKind::BudgetUsd,
        AttachmentKind::CriticalSystemReminder,
        AttachmentKind::SlashCommandMetadata,
        AttachmentKind::EditedImageFile,
        AttachmentKind::EditedTextFile,
        AttachmentKind::OpenedFileInIde,
        AttachmentKind::OutputStyle,
        AttachmentKind::PlanMode,
        AttachmentKind::PlanModeExit,
        AttachmentKind::PlanModeReentry,
        AttachmentKind::StructuredOutput,
        AttachmentKind::TeamContext,
        AttachmentKind::TodoReminder,
        AttachmentKind::ContextEfficiency,
        AttachmentKind::DeferredToolsDelta,
        AttachmentKind::McpInstructionsDelta,
        AttachmentKind::CompanionIntro,
        AttachmentKind::TokenUsage,
        AttachmentKind::UltrathinkEffort,
        AttachmentKind::MaxTurnsReached,
        AttachmentKind::TaskReminder,
        AttachmentKind::AutoMode,
        AttachmentKind::AutoModeExit,
        AttachmentKind::OutputTokenUsage,
        AttachmentKind::VerifyPlanReminder,
        AttachmentKind::CurrentSessionMemory,
        AttachmentKind::CompactionReminder,
        AttachmentKind::DateChange,
    ];
    for k in ts_null_rendering {
        assert!(
            !k.renders_in_transcript(),
            "TS NULL_RENDERING kind {k:?} must return renders_in_transcript() == false; \
             did TS upstream change or Rust predicate drift?"
        );
    }
}

/// TS parity guard: every kind whose TS `normalizeAttachmentForAPI`
/// returns `[]` unconditionally must return `is_api_visible() == false`
/// in coco-rs. Mirrors `utils/messages.ts:4250-4261` + early-return cases
/// (`dynamic_skill`) + case-less variants that fall through to the
/// default `return []` (`max_turns_reached` / etc.).
#[test]
fn is_api_visible_matches_ts_normalize_attachment_for_api_returns_empty() {
    let ts_api_hidden = [
        // `case '...': return []` block at messages.ts:4250-4261
        AttachmentKind::AlreadyReadFile,
        AttachmentKind::CommandPermissions,
        AttachmentKind::EditedImageFile,
        AttachmentKind::HookCancelled,
        AttachmentKind::HookErrorDuringExecution,
        AttachmentKind::HookNonBlockingError,
        AttachmentKind::HookSystemMessage,
        AttachmentKind::StructuredOutput,
        AttachmentKind::HookPermissionDecision,
        // Early `return []` inside the `dynamic_skill` case.
        AttachmentKind::DynamicSkill,
        // Feature-gated off by default in external builds →
        // `normalizeAttachmentForAPI` returns `[]`.
        AttachmentKind::ContextEfficiency,
        // Variants with no `case` — fall through to `logAntError + return []`.
        AttachmentKind::MaxTurnsReached,
        AttachmentKind::CurrentSessionMemory,
        AttachmentKind::TeammateShutdownBatch,
        AttachmentKind::BagelConsole,
    ];
    for k in ts_api_hidden {
        assert!(
            !k.is_api_visible(),
            "TS API-hidden kind {k:?} must return is_api_visible() == false; \
             did TS upstream change or Rust predicate drift?"
        );
    }
}

#[test]
fn sdk_consumption_matches_ts_query_engine_special_cases() {
    // TS `QueryEngine.ts` has attachment branches for:
    // - `structured_output` -> final SDK result `structured_output`
    // - `max_turns_reached` -> terminal `error_max_turns` result
    // - `queued_command` -> SDK user replay when replay mode is enabled
    assert_eq!(
        AttachmentKind::StructuredOutput.sdk_consumption(),
        SdkConsumption::ResultField {
            field: "structured_output"
        }
    );
    assert_eq!(
        AttachmentKind::MaxTurnsReached.sdk_consumption(),
        SdkConsumption::TerminalResult {
            subtype: "error_max_turns"
        }
    );
    assert_eq!(
        AttachmentKind::QueuedCommand.sdk_consumption(),
        SdkConsumption::ReplayUserMessage
    );

    for k in AttachmentKind::all() {
        let expected = matches!(
            k,
            AttachmentKind::StructuredOutput
                | AttachmentKind::MaxTurnsReached
                | AttachmentKind::QueuedCommand
        );
        assert_eq!(
            k.is_sdk_consumed(),
            expected,
            "SDK consumption drift for {k:?}"
        );
    }
}

#[test]
fn attachment_event_silent_constructor() {
    let e = AttachmentEvent::silent(
        AttachmentKind::HookCancelled,
        serde_json::json!({"hook_name": "prestop"}),
    );
    assert_eq!(e.kind, AttachmentKind::HookCancelled);
    assert!(e.is_meta);
    assert_eq!(e.payload, serde_json::json!({"hook_name": "prestop"}));
}

#[test]
fn attachment_event_silent_marker_is_null_payload() {
    let e = AttachmentEvent::silent_marker(AttachmentKind::StructuredOutput);
    assert!(e.payload.is_null());
    assert!(e.is_meta);
}

#[test]
fn attachment_event_visible_clears_meta() {
    let e = AttachmentEvent::visible(AttachmentKind::EditedTextFile, serde_json::json!({}));
    assert!(!e.is_meta);
}

#[test]
fn attachment_event_serde_roundtrip() {
    let original = AttachmentEvent::silent(
        AttachmentKind::CommandPermissions,
        serde_json::json!({"rule": "bash:ls"}),
    );
    let json = serde_json::to_string(&original).unwrap();
    let back: AttachmentEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(original, back);
}

/// Guard: every `Coverage::SilentEvent` kind can actually be packaged
/// into an `AttachmentEvent`. This is the cross-crate contract — owner
/// crates (hooks / permissions / commands / tools / skills) produce
/// these via `AttachmentEvent::silent(kind, payload)`.
#[test]
fn every_silent_event_kind_is_constructible() {
    for k in AttachmentKind::all() {
        if matches!(k.coverage(), Coverage::SilentEvent { .. }) {
            let e = AttachmentEvent::silent(*k, serde_json::Value::Null);
            assert_eq!(e.kind, *k);
            assert!(e.is_meta);
        }
    }
}
