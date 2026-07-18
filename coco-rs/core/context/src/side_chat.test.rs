use super::*;
use pretty_assertions::assert_eq;

#[test]
fn max_inherited_tokens_halves_then_caps() {
    // Small window: half the window wins.
    assert_eq!(max_inherited_tokens(16_000), 7_808);
    // Large window: the absolute cap wins.
    assert_eq!(
        max_inherited_tokens(1_000_000),
        MAX_INHERITED_TOKENS_ABSOLUTE_CAP
    );
    // Exactly at the cap boundary.
    assert_eq!(
        max_inherited_tokens(65_536),
        MAX_INHERITED_TOKENS_ABSOLUTE_CAP
    );
    // Degenerate windows clamp at zero.
    assert_eq!(max_inherited_tokens(0), 0);
    assert_eq!(max_inherited_tokens(-10), 0);
}

#[test]
fn boundary_fragment_renders_enforced_facts_only() {
    let text = SideChatBoundaryFragment.render();
    assert!(text.contains("reference material"));
    assert!(text.contains("do not modify the parent conversation"));
    assert!(text.contains("read-only"));
    assert!(text.contains("permission"));
    // Must NOT promise auto-approval or claim the parent is frozen.
    assert!(!text.to_lowercase().contains("auto-approve"));
    assert!(SideChatBoundaryFragment.estimated_tokens() > 0);
}

#[test]
fn empty_bounded_context_is_full_prefix() {
    let ctx = BoundedContext::empty();
    assert!(ctx.is_empty());
    assert_eq!(ctx.estimated_tokens(), 0);
    assert!(ctx.fidelity().is_full_prefix());
}

#[test]
fn bounded_fallback_is_not_full_prefix() {
    let fidelity = ContextFidelity::BoundedFallback { omitted_groups: 3 };
    assert!(!fidelity.is_full_prefix());
}

fn user(text: &str) -> std::sync::Arc<coco_messages::Message> {
    std::sync::Arc::new(coco_messages::create_user_message(text))
}

fn assistant() -> std::sync::Arc<coco_messages::Message> {
    std::sync::Arc::new(coco_messages::create_assistant_message(
        Vec::new(),
        "test-model",
        coco_types::TokenUsage::default(),
    ))
}

#[test]
fn under_budget_capture_is_full_prefix_verbatim() {
    let msgs = vec![user("hello there"), user("second turn")];
    let total = coco_messages::estimate_tokens_for_messages(&msgs);
    let bc = super::capture_bounded_context(&msgs, total + 100).expect("fits");
    assert!(bc.fidelity().is_full_prefix());
    assert_eq!(bc.messages().len(), 2);
    assert_eq!(bc.estimated_tokens(), total);
}

#[test]
fn over_budget_capture_drops_oldest_whole_turns() {
    let big = "x".repeat(4000);
    let msgs = vec![user(&big), user(&big), user(&big)];
    // A budget that fits the two newest turns but not all three.
    let marker = super::omission_message(1);
    let budget =
        coco_messages::estimate_tokens_for_messages(&msgs[1..]) + super::message_tokens(&marker);
    let bc = super::capture_bounded_context(&msgs, budget).expect("fits two plus marker");
    match bc.fidelity() {
        ContextFidelity::BoundedFallback { omitted_groups } => assert_eq!(*omitted_groups, 1),
        other => panic!("expected bounded fallback, got {other:?}"),
    }
    assert_eq!(bc.messages().len(), 3);
    assert!(bc.estimated_tokens() <= budget);
}

#[test]
fn capture_never_starts_mid_group() {
    let big = "x".repeat(4000);
    // Two turns, each a User followed by an Assistant response.
    let msgs = vec![user(&big), assistant(), user(&big), assistant()];
    let budget = coco_messages::estimate_tokens_for_messages(&msgs[2..])
        + super::message_tokens(&super::omission_message(1));
    let bc = super::capture_bounded_context(&msgs, budget).expect("fits newest group plus marker");
    assert!(
        matches!(bc.messages()[1].as_ref(), coco_messages::Message::User(_)),
        "the kept slice must begin at a User boundary, never a tool/assistant continuation"
    );
    assert!(matches!(
        bc.messages()[0].as_ref(),
        coco_messages::Message::Attachment(_)
    ));
    assert_eq!(bc.messages().len(), 3);
}

#[test]
fn single_oversized_turn_is_an_error() {
    let big = "x".repeat(8000);
    let msgs = vec![user(&big)];
    let err = super::capture_bounded_context(&msgs, 10).expect_err("too large");
    assert!(matches!(
        err,
        crate::ContextError::SideChatContextTooLarge { .. }
    ));
}

#[test]
fn oversized_fragment_is_rejected_even_when_total_budget_is_larger() {
    let msgs = vec![user(&"x".repeat(40_000))];
    let err = super::capture_bounded_context(&msgs, 20_000).expect_err("fragment cap");
    assert!(matches!(
        err,
        crate::ContextError::SideChatContextTooLarge { .. }
    ));
}

#[test]
fn system_injected_user_message_does_not_create_a_turn_boundary() {
    let old = user(&"o".repeat(4_000));
    let mut injected = coco_messages::create_user_message(&"i".repeat(4_000));
    let coco_messages::Message::User(injected_user) = &mut injected else {
        unreachable!()
    };
    injected_user.origin = Some(coco_types::MessageOrigin::SystemInjected);
    let newest = user(&"n".repeat(4_000));
    let msgs = vec![old, Arc::new(injected), newest];
    let budget = super::message_tokens(&super::omission_message(1))
        + coco_messages::estimate_tokens_for_messages(&msgs[2..]);

    let captured = super::capture_bounded_context(&msgs, budget).expect("newest semantic turn");
    assert_eq!(captured.messages().len(), 2);
    assert!(matches!(
        captured.fidelity(),
        ContextFidelity::BoundedFallback { omitted_groups: 1 }
    ));
}
