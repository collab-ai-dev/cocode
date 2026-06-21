use super::*;

#[test]
fn test_teammate_prompt_addendum_content() {
    // Must match TS TEAMMATE_SYSTEM_PROMPT_ADDENDUM exactly
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("Agent Teammate Communication"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("SendMessage tool"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("to: \"<name>\""));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("to: \"*\""));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("not visible to others"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("MUST use the SendMessage tool"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("team lead"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("task system"));
    // Shutdown protocol guidance (gap 6) — teammates must know to reply
    // with a shutdown_response when the leader requests shutdown.
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("shutdown_request"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("shutdown_response"));
    assert!(TEAMMATE_PROMPT_ADDENDUM.contains("request_id"));
}

#[test]
fn test_permission_poll_interval() {
    assert_eq!(PERMISSION_POLL_INTERVAL_MS, 500);
}

#[test]
fn test_build_teammate_system_prompt_default() {
    let prompt =
        build_teammate_system_prompt(Some("You are helpful."), None, SystemPromptMode::Default);
    assert!(prompt.contains("You are helpful."));
    assert!(prompt.contains("Agent Teammate Communication"));
}

#[test]
fn test_build_teammate_system_prompt_default_no_base() {
    let prompt = build_teammate_system_prompt(None, None, SystemPromptMode::Default);
    assert!(prompt.contains("Agent Teammate Communication"));
}

#[test]
fn test_build_teammate_system_prompt_replace() {
    let prompt =
        build_teammate_system_prompt(Some("base"), Some("custom only"), SystemPromptMode::Replace);
    assert_eq!(prompt, "custom only");
    assert!(!prompt.contains("Agent Teammate Communication"));
}

#[test]
fn test_build_teammate_system_prompt_append() {
    let prompt =
        build_teammate_system_prompt(Some("base"), Some("custom"), SystemPromptMode::Append);
    assert!(prompt.contains("base"));
    assert!(prompt.contains("Agent Teammate Communication"));
    assert!(prompt.contains("custom"));
    let base_pos = prompt.find("base").unwrap();
    let addendum_pos = prompt.find("Agent Teammate Communication").unwrap();
    let custom_pos = prompt.rfind("custom").unwrap();
    assert!(base_pos < addendum_pos);
    assert!(addendum_pos < custom_pos);
}
