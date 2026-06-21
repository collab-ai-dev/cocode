//! Tests for the rename helpers.
//!
//! The LLM path requires real provider credentials and is exercised
//! via integration tests; here we only assert the pure pieces:
//! - `AutoRenameError::user_message` returns deterministic prose.

use super::*;

#[test]
fn auto_rename_error_user_message_no_conversation() {
    let msg = AutoRenameError::NoConversation.user_message();
    assert!(msg.contains("conversation"));
    assert!(msg.contains("/rename <name>"));
}

#[test]
fn auto_rename_error_user_message_llm_failed() {
    let msg = AutoRenameError::LlmFailed.user_message();
    assert!(msg.contains("Couldn't"));
    assert!(msg.contains("/rename <name>"));
}
