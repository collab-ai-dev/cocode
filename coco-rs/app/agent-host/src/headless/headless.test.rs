use super::RunChatOptions;
use super::parse_headless_goal_slash;
use super::run::terminal_failure_message;
use super::run_chat_with_options;
use crate::AgentHostOptions;

struct ConfigDirGuard {
    previous: Option<std::ffi::OsString>,
}

impl ConfigDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os(coco_utils_common::COCO_CONFIG_DIR_ENV);
        // SAFETY: tests using this helper hold the crate-wide config env lock
        // for the guard's lifetime.
        unsafe { std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, path) };
        Self { previous }
    }
}

impl Drop for ConfigDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                // SAFETY: the crate-wide config env lock is held.
                unsafe { std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, value) };
            }
            None => {
                // SAFETY: the crate-wide config env lock is held.
                unsafe { std::env::remove_var(coco_utils_common::COCO_CONFIG_DIR_ENV) };
            }
        }
    }
}

#[test]
fn parse_headless_goal_slash_accepts_exact_goal_command() {
    assert_eq!(parse_headless_goal_slash("/goal"), Some(""));
    assert_eq!(parse_headless_goal_slash("  /goal   "), Some(""));
    assert_eq!(
        parse_headless_goal_slash("/goal finish migration"),
        Some("finish migration")
    );
}

#[test]
fn parse_headless_goal_slash_rejects_other_inputs() {
    assert_eq!(parse_headless_goal_slash("goal finish"), None);
    assert_eq!(parse_headless_goal_slash("/goalx finish"), None);
    assert_eq!(parse_headless_goal_slash("/loop 5m /goal done"), None);
}

#[test]
fn typed_turn_failure_is_a_headless_process_failure() {
    let error = coco_event_types::ErrorPayload {
        code: coco_event_types::ErrorCode::Provider,
        message: "provider returned no terminal content".into(),
    };
    let outcome =
        coco_event_types::TurnEndedParams::failed("failed-turn".into(), None, error.clone())
            .outcome;
    let session_result = coco_event_types::SessionResultParams {
        session_id: coco_types::SessionId::generate(),
        total_turns: 1,
        duration_ms: 0,
        duration_api_ms: 0,
        is_error: true,
        stop_reason: "error_empty_response_retries".into(),
        total_cost_usd: 0.0,
        usage: coco_types::TokenUsage::default(),
        model_usage: Default::default(),
        permission_denials: Vec::new(),
        result: None,
        errors: vec![error.message],
        structured_output: None,
        fast_mode_state: None,
        num_api_calls: Some(4),
    };

    assert_eq!(
        terminal_failure_message(&outcome, &session_result).as_deref(),
        Some("provider returned no terminal content")
    );
}

#[test]
fn run_chat_with_options_requires_explicit_cwd_without_cli_cwd() {
    let cli = AgentHostOptions::default();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let err = rt
        .block_on(run_chat_with_options(
            &cli,
            Some("/goal"),
            RunChatOptions::default(),
        ))
        .expect_err("run_chat_with_options should require explicit cwd");

    assert!(
        err.to_string().contains("requires RunChatOptions::cwd"),
        "unexpected error: {err}"
    );
}

#[test]
#[serial_test::serial(config_env)]
fn local_goal_print_run_writes_resumable_zero_turn_transcript() {
    let _lock = crate::test_support::CONFIG_ENV_LOCK.blocking_lock();
    let config_home = tempfile::tempdir().expect("config home");
    let cwd = tempfile::tempdir().expect("cwd");
    let _guard = ConfigDirGuard::set(config_home.path());
    let cli = AgentHostOptions {
        session_id: Some("zero-model-turn-session".to_string()),
        ..AgentHostOptions::default()
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let outcome = rt
        .block_on(run_chat_with_options(
            &cli,
            Some("/goal"),
            RunChatOptions {
                cwd: Some(cwd.path().to_path_buf()),
                ..Default::default()
            },
        ))
        .expect("local goal run");

    assert_eq!(outcome.turns, 0);
    let paths = coco_paths::ProjectPaths::new(config_home.path().to_path_buf(), cwd.path());
    let transcript = paths.transcript("zero-model-turn-session");
    assert!(
        coco_session::recovery::can_resume_session(&transcript),
        "local no-model-turn run must create a resumable transcript at {}",
        transcript.display()
    );
    let conversation = coco_session::recovery::load_conversation_for_resume(&transcript)
        .expect("zero-turn transcript should load");
    assert_eq!(conversation.turn_count, 0);
    assert!(
        !conversation.messages.is_empty(),
        "resume should recover the local slash-command transcript"
    );
}
