use std::sync::Mutex;

use super::RunChatOptions;
use super::parse_headless_goal_slash;
use super::run_chat_with_options;
use crate::AgentHostOptions;

static CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

struct ConfigDirGuard {
    previous: Option<std::ffi::OsString>,
}

impl ConfigDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os(coco_utils_common::COCO_CONFIG_DIR_ENV);
        // SAFETY: tests using this helper hold CONFIG_ENV_LOCK for the
        // guard's lifetime.
        unsafe { std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, path) };
        Self { previous }
    }
}

impl Drop for ConfigDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                // SAFETY: tests using this helper hold CONFIG_ENV_LOCK.
                unsafe { std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, value) };
            }
            None => {
                // SAFETY: tests using this helper hold CONFIG_ENV_LOCK.
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
fn local_goal_print_run_writes_resumable_zero_turn_transcript() {
    let _lock = CONFIG_ENV_LOCK.lock().expect("config env lock");
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
