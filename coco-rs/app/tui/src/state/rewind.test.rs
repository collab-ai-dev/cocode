use super::*;

#[test]
fn test_build_restore_options_with_file_changes() {
    let opts = build_restore_options(
        /*file_history_enabled*/ true, /*has_file_changes*/ true,
        /*allow_summarize_up_to*/ false,
    );
    assert_eq!(
        opts,
        vec![
            RestoreType::Both,
            RestoreType::ConversationOnly,
            RestoreType::CodeOnly,
            RestoreType::SummarizeFrom { feedback: None },
            RestoreType::Nevermind,
        ]
    );
}

#[test]
fn test_build_restore_options_no_file_changes() {
    let opts = build_restore_options(
        /*file_history_enabled*/ true, /*has_file_changes*/ false,
        /*allow_summarize_up_to*/ false,
    );
    assert_eq!(
        opts,
        vec![
            RestoreType::ConversationOnly,
            RestoreType::SummarizeFrom { feedback: None },
            RestoreType::Nevermind,
        ]
    );
}

#[test]
fn test_build_restore_options_file_history_disabled() {
    let opts = build_restore_options(
        /*file_history_enabled*/ false, /*has_file_changes*/ false,
        /*allow_summarize_up_to*/ false,
    );
    assert_eq!(
        opts,
        vec![
            RestoreType::ConversationOnly,
            RestoreType::SummarizeFrom { feedback: None },
            RestoreType::Nevermind,
        ]
    );
}

#[test]
fn test_build_restore_options_summarize_up_to_gated() {
    let opts = build_restore_options(true, true, true);
    assert!(opts.contains(&RestoreType::SummarizeUpTo { feedback: None }));
}

#[test]
fn test_build_restore_options_nevermind_is_last() {
    let opts = build_restore_options(true, true, true);
    assert_eq!(opts.last(), Some(&RestoreType::Nevermind));
}
