use super::*;

#[test]
fn gpt4o_is_not_reasoning() {
    let caps = get_capabilities("gpt-4o");
    assert!(!caps.is_reasoning_model);
    assert_eq!(caps.system_message_mode, SystemMessageMode::System);
}

#[test]
fn o3_is_reasoning() {
    let caps = get_capabilities("o3");
    assert!(caps.is_reasoning_model);
    assert_eq!(caps.system_message_mode, SystemMessageMode::Developer);
    assert!(caps.supports_flex_processing);
    assert!(caps.supports_priority_processing);
}

#[test]
fn o4_mini_is_reasoning() {
    let caps = get_capabilities("o4-mini-2025-04-16");
    assert!(caps.is_reasoning_model);
    assert!(caps.supports_flex_processing);
}

#[test]
fn gpt5_is_reasoning() {
    let caps = get_capabilities("gpt-5");
    assert!(caps.is_reasoning_model);
    assert!(caps.supports_flex_processing);
    assert!(caps.supports_priority_processing);
}

#[test]
fn gpt5_chat_is_not_reasoning() {
    let caps = get_capabilities("gpt-5-chat");
    assert!(!caps.is_reasoning_model);
    assert_eq!(caps.system_message_mode, SystemMessageMode::System);
}

#[test]
fn o1_is_reasoning() {
    let caps = get_capabilities("o1");
    assert!(caps.is_reasoning_model);
    assert!(!caps.supports_flex_processing);
}

#[test]
fn gpt5_1_supports_non_reasoning_params() {
    let caps = get_capabilities("gpt-5.1");
    assert!(caps.supports_non_reasoning_params_with_no_effort);
    assert!(caps.is_reasoning_model);
}

#[test]
fn gpt4_supports_priority_processing() {
    let caps = get_capabilities("gpt-4-turbo");
    assert!(caps.supports_priority_processing);
    assert!(!caps.supports_flex_processing);
}
