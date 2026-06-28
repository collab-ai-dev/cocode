use super::*;

#[test]
fn feedback_usage_requires_description() {
    let out = handler("");
    assert!(out.contains("Usage: /feedback"));
    assert!(out.contains("Logs are not included by default"));
}

#[test]
fn feedback_default_omits_logs_and_includes_runtime() {
    let out = handler("Something went wrong");
    assert!(out.contains("https://github.com/collab-ai-dev/cocode/issues/new"));
    assert!(out.contains("Logs included: no"));
    assert!(out.contains("Version%3A"));
    assert!(out.contains("Commit%3A"));
    assert!(!out.contains("secret_token"));
}

#[test]
fn feedback_with_logs_redacts_current_log_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("custom.log");
    let dated = PathBuf::from(format!(
        "{}.{}",
        base.display(),
        chrono::Utc::now().format("%Y-%m-%d")
    ));
    std::fs::write(
        &dated,
        "before\napi_key = \"super_secret_value_12345\"\nafter\n",
    )
    .expect("write log");

    let chosen = rotating_candidates(&base)
        .into_iter()
        .find(|path| path.is_file())
        .expect("dated log candidate");
    let tail = read_tail(&chosen, LOG_TAIL_BYTES).expect("read tail");
    let redacted = redact_secrets(&tail);

    assert!(redacted.contains("[REDACTED_SECRET]"));
    assert!(!redacted.contains("super_secret_value"));
}

#[test]
fn parse_args_treats_logs_as_opt_in_flag() {
    assert_eq!(
        parse_args("--with-logs broken thing"),
        FeedbackArgs {
            include_logs: true,
            description: "broken thing".to_string()
        }
    );
    assert_eq!(
        parse_args("--with-logs --no-logs broken thing"),
        FeedbackArgs {
            include_logs: false,
            description: "broken thing".to_string()
        }
    );
}
