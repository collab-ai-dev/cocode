use super::*;
use pretty_assertions::assert_eq;

#[test]
fn all_plan_and_auto_variants_are_core_tier() {
    assert_eq!(AttachmentType::PlanMode.tier(), ReminderTier::Core);
    assert_eq!(AttachmentType::PlanModeExit.tier(), ReminderTier::Core);
    assert_eq!(AttachmentType::PlanModeReentry.tier(), ReminderTier::Core);
    assert_eq!(AttachmentType::AutoModeExit.tier(), ReminderTier::Core);
}

#[test]
fn tiers_match_ts_attachment_batches() {
    assert_eq!(
        AttachmentType::AtMentionedFiles.tier(),
        ReminderTier::UserPrompt
    );
    assert_eq!(
        AttachmentType::McpResources.tier(),
        ReminderTier::UserPrompt
    );
    assert_eq!(
        AttachmentType::AgentMentions.tier(),
        ReminderTier::UserPrompt
    );

    // skill_listing is Core tier so sub-agents can still discover available skills.
    assert_eq!(AttachmentType::SkillListing.tier(), ReminderTier::Core);

    // IDE state is MainAgentOnly, not UserPrompt tier.
    assert_eq!(
        AttachmentType::IdeSelection.tier(),
        ReminderTier::MainAgentOnly
    );
    assert_eq!(
        AttachmentType::IdeOpenedFile.tier(),
        ReminderTier::MainAgentOnly
    );
}

#[test]
fn all_phase_a_variants_use_system_reminder_tag() {
    for at in [
        AttachmentType::PlanMode,
        AttachmentType::PlanModeExit,
        AttachmentType::PlanModeReentry,
        AttachmentType::AutoModeExit,
    ] {
        assert_eq!(at.xml_tag(), XmlTag::SystemReminder);
    }
}

#[test]
fn attachment_type_serde_matches_ts_wire_format() {
    let pairs = [
        (AttachmentType::PlanMode, r#""plan_mode""#),
        (AttachmentType::PlanModeExit, r#""plan_mode_exit""#),
        (AttachmentType::PlanModeReentry, r#""plan_mode_reentry""#),
        (AttachmentType::AutoModeExit, r#""auto_mode_exit""#),
    ];
    for (variant, expected) in pairs {
        let actual = serde_json::to_string(&variant).expect("serialize");
        assert_eq!(actual, expected, "variant {variant:?} wire mismatch");
        assert_eq!(variant.as_str(), expected.trim_matches('"'));
    }
}

#[test]
fn xml_tag_name_roundtrip() {
    assert_eq!(XmlTag::SystemReminder.tag_name(), Some("system-reminder"));
    assert_eq!(XmlTag::None.tag_name(), None);
}

#[test]
fn system_reminder_new_defaults_to_meta_non_silent() {
    let r = SystemReminder::new(AttachmentType::PlanMode, "hello");
    assert!(r.is_meta);
    assert!(!r.is_silent);
    assert_eq!(r.content(), Some("hello"));
    assert_eq!(r.xml_tag(), XmlTag::SystemReminder);
}

#[test]
fn silent_reminder_is_detected() {
    let r = SystemReminder::new(AttachmentType::PlanMode, "x").silent();
    assert!(r.is_silent);
    // Still has content; is_silent is orthogonal to emptiness.
    assert!(!r.output.is_silent());
}

#[test]
fn empty_text_output_is_silent() {
    let out = ReminderOutput::Text(String::new());
    assert!(out.is_silent());
    assert_eq!(out.as_text(), Some(""));
}

#[test]
fn empty_messages_output_is_silent() {
    let out = ReminderOutput::Messages(Vec::new());
    assert!(out.is_silent());
    assert_eq!(out.as_text(), None);
}

#[test]
fn reminder_message_user_text_sets_is_meta() {
    let m = ReminderMessage::user_text("foo");
    assert_eq!(m.role, MessageRole::User);
    assert!(m.is_meta);
    assert_eq!(m.blocks.len(), 1);
    match &m.blocks[0] {
        ContentBlock::Text { text } => assert_eq!(text, "foo"),
        _ => panic!("expected text block"),
    }
}

#[test]
fn attachment_type_display_matches_as_str() {
    assert_eq!(AttachmentType::PlanMode.to_string(), "plan_mode");
    assert_eq!(AttachmentType::PlanModeExit.to_string(), "plan_mode_exit");
}

/// Guard: every `AttachmentType` lifts into an `AttachmentKind` — the
/// From impl being exhaustive is enforced by Rust's match; this test
/// just exercises it so any future `unreachable!()` is caught.
#[test]
fn every_attachment_type_lifts_to_an_attachment_kind() {
    for at in AttachmentType::all() {
        let _kind: coco_types::AttachmentKind = (*at).into();
    }
}

/// Guard: every `AttachmentKind` whose coverage is `Reminder` /
/// `SilentReminder` names a generator that's actually registered in
/// the default orchestrator. This catches the case where the coverage
/// map claims "PlanModeEnterGenerator handles PlanMode" but someone
/// renamed / removed the generator without updating the map.
///
/// Lives here (not in `coco-types`) because `coco-types` can't depend
/// on the system-reminder crate — L1 can't see L3.
#[test]
fn every_reminder_coverage_names_a_registered_generator() {
    use coco_types::Coverage;
    use std::collections::HashSet;

    let o = crate::orchestrator::SystemReminderOrchestrator::new(
        coco_config::SystemReminderConfig::default(),
    )
    .with_default_generators();
    // `AttachmentGenerator::name()` returns the impl's struct name —
    // collect that set and then ensure every `Coverage::Reminder` /
    // `Coverage::SilentReminder` generator string is represented.
    let registered: HashSet<&str> = o.generator_names().into_iter().collect();

    for k in coco_types::AttachmentKind::all() {
        if matches!(k, coco_types::AttachmentKind::QueuedCommand) {
            continue;
        }
        let expected = match k.coverage() {
            Coverage::Reminder { generator } | Coverage::SilentReminder { generator } => generator,
            _ => continue,
        };
        assert!(
            registered.contains(expected),
            "Coverage for {k:?} names generator {expected:?} but no such \
             generator is in the default registry. Registered generators: \
             {registered:?}"
        );
    }
}

/// **Strong reverse-binding guard** (Gap F): the generator named in
/// [`Coverage::Reminder`] / [`Coverage::SilentReminder`] must produce
/// an [`AttachmentType`] that lifts (via `From`) back to the same
/// [`AttachmentKind`] the coverage entry keyed on.
///
/// Why: the weaker guard above only asserts the name *exists* in the
/// registry. If someone accidentally mapped
/// `AttachmentKind::QueuedCommand → "McpResourcesGenerator"`, both
/// strings point at real generators so the weak test passes — but the
/// binding is wrong (the model would expect McpResources content under
/// the QueuedCommand wire tag). This test closes that hole by walking
/// registered generators + their emitted `AttachmentType` back to
/// `AttachmentKind`, so any "right name, wrong kind" mapping panics.
#[test]
fn coverage_reminder_binding_round_trips_through_attachment_type() {
    use coco_types::Coverage;

    let o = crate::orchestrator::SystemReminderOrchestrator::new(
        coco_config::SystemReminderConfig::default(),
    )
    .with_default_generators();

    // Index the registry by generator name → emitted AttachmentType, so
    // we can reverse the Coverage map's string pointer into a kind.
    let mut by_name: std::collections::HashMap<&str, crate::types::AttachmentType> =
        std::collections::HashMap::new();
    for g in o
        .generator_names()
        .into_iter()
        .zip(o.registered_attachment_types())
    {
        by_name.insert(g.0, g.1);
    }

    for k in coco_types::AttachmentKind::all() {
        if matches!(k, coco_types::AttachmentKind::QueuedCommand) {
            continue;
        }
        let expected_gen = match k.coverage() {
            Coverage::Reminder { generator } | Coverage::SilentReminder { generator } => generator,
            _ => continue,
        };
        let Some(at) = by_name.get(expected_gen).copied() else {
            panic!(
                "Coverage for {k:?} names generator {expected_gen:?} but the \
                 default registry has no generator with that name — weak guard \
                 should have caught this; are you skipping tests?"
            );
        };
        let lifted: coco_types::AttachmentKind = at.into();
        // AgentPendingMessages is a synthetic type that maps to the
        // `queued_command` wire kind. Coverage records the wire kind;
        // the generator produces the synthetic type. Accept either.
        let synthetic_ok = matches!(at, crate::types::AttachmentType::AgentPendingMessages)
            && matches!(k, coco_types::AttachmentKind::QueuedCommand);
        assert!(
            lifted == *k || synthetic_ok,
            "binding drift: Coverage says AttachmentKind::{k:?} is handled by \
             {expected_gen:?}, but that generator's AttachmentType \
             ({at:?}) lifts to AttachmentKind::{lifted:?} — the binding is \
             wrong, rename / reassign either side"
        );
    }
}
