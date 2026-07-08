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

#[test]
fn normalize_resolved_name_trims_and_rejects_empty() {
    assert_eq!(
        normalize_resolved_name("  phase-b-cleanup  ".to_string()).unwrap(),
        "phase-b-cleanup"
    );

    let err = normalize_resolved_name("   ".to_string()).unwrap_err();
    assert!(matches!(err, RenamePersistenceError::EmptyName));
}

#[test]
fn rename_persistence_error_messages_match_request_semantics() {
    assert_eq!(
        RenamePersistenceError::EmptyName.user_message(),
        "session/rename requires a non-empty name"
    );
    assert!(
        RenamePersistenceError::TranscriptNotFound
            .user_message()
            .starts_with("Cannot rename:")
    );
}
