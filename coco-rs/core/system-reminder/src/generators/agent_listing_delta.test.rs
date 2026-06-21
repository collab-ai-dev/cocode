use super::*;
use crate::generator::AgentListingDeltaInfo;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn skips_when_no_delta() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .agent_listing_delta(None)
        .build();
    assert!(
        AgentListingDeltaGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn initial_emits_header_plus_concurrency_note() {
    let c = SystemReminderConfig::default();
    let info = AgentListingDeltaInfo {
        added_lines: vec![
            "- explore: Parallel code exploration".to_string(),
            "- plan: Drafts an implementation plan".to_string(),
        ],
        removed_types: vec![],
        is_initial: true,
        show_concurrency_note: true,
    };
    let ctx = GeneratorContext::builder(&c)
        .agent_listing_delta(Some(info))
        .build();
    let text = AgentListingDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("Available agent types for the Agent tool:"));
    assert!(text.contains("- explore: Parallel code exploration"));
    assert!(text.contains("Launch multiple agents concurrently whenever possible"));
}

#[tokio::test]
async fn non_initial_uses_new_header_and_still_includes_concurrency_note() {
    // The concurrency hint fires on every delta, not just the initial one.
    // We have no subscription-tier flag, so we surface the reminder whenever
    // new agent types arrive (which is the only time the renderer runs).
    let c = SystemReminderConfig::default();
    let info = AgentListingDeltaInfo {
        added_lines: vec!["- foo: New agent".to_string()],
        removed_types: vec!["bar".to_string()],
        is_initial: false,
        show_concurrency_note: true,
    };
    let ctx = GeneratorContext::builder(&c)
        .agent_listing_delta(Some(info))
        .build();
    let text = AgentListingDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("New agent types are now available for the Agent tool:"));
    assert!(
        text.contains("Launch multiple agents concurrently"),
        "concurrency note must fire on every delta in coco-rs"
    );
    // Removed section lists each with `- ` prefix.
    assert!(text.contains("- bar"));
    assert!(text.contains("The following agent types are no longer available:"));
}

#[tokio::test]
async fn omits_concurrency_note_when_flag_is_false() {
    // Defensive: callers that explicitly pass `show_concurrency_note:
    // false` still suppress the hint (e.g. future settings-driven
    // opt-out). compute_agents_delta itself sets it to `true`
    // unconditionally today.
    let c = SystemReminderConfig::default();
    let info = AgentListingDeltaInfo {
        added_lines: vec!["- foo: New agent".to_string()],
        removed_types: vec![],
        is_initial: false,
        show_concurrency_note: false,
    };
    let ctx = GeneratorContext::builder(&c)
        .agent_listing_delta(Some(info))
        .build();
    let text = AgentListingDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("New agent types are now available for the Agent tool:"));
    assert!(!text.contains("Launch multiple agents concurrently"));
}

#[tokio::test]
async fn emits_standalone_concurrency_note_on_initial_even_without_changes() {
    let c = SystemReminderConfig::default();
    let info = AgentListingDeltaInfo {
        added_lines: vec![],
        removed_types: vec![],
        is_initial: true,
        show_concurrency_note: true,
    };
    let ctx = GeneratorContext::builder(&c)
        .agent_listing_delta(Some(info))
        .build();
    let r = AgentListingDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits note-only");
    let text = r.content().unwrap();
    assert_eq!(
        text,
        "Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses."
    );
}
