use super::*;

#[test]
fn prefix_input_caps_new_and_insert_at_the_context_budget() {
    let limit = coco_types::MAX_PERMISSION_FEEDBACK_BYTES;
    let mut input = PrefixInputState::new("a".repeat(limit + 8));

    assert_eq!(input.value.len(), limit);
    assert_eq!(input.cursor, limit);
    input.insert('b');
    assert_eq!(input.value.len(), limit);
}

#[test]
fn prefix_input_cap_preserves_utf8_boundaries() {
    let mut input = PrefixInputState::new("🙂".repeat(300));

    assert!(input.value.len() <= coco_types::MAX_PERMISSION_FEEDBACK_BYTES);
    assert!(input.value.chars().all(|ch| ch == '🙂'));
    input.insert('🙂');
    assert!(input.value.len() <= coco_types::MAX_PERMISSION_FEEDBACK_BYTES);
}
