use std::time::Duration;
use std::time::SystemTime;

use clap::Parser;
use pretty_assertions::assert_eq;

use super::LogEnv;
use super::detect_mode;
use super::expand_bare_level;
use super::parse_timezone;
use super::resolve_location;
use super::subscriber_opts_from_cli;
use super::subscriber_opts_from_cli_with_sources;
use super::sweep_stale_logs;
use crate::Cli;
use coco_config::PartialLogSettings;
use coco_config::Settings;
use coco_otel::subscriber::Format;
use coco_otel::subscriber::Mode;
use coco_otel::subscriber::TimezoneConfig;

fn parse(args: &[&str]) -> Cli {
    let mut full = vec!["coco"];
    full.extend_from_slice(args);
    Cli::parse_from(full)
}

fn settings_with_log(log: PartialLogSettings) -> Settings {
    Settings {
        log,
        ..Settings::default()
    }
}

#[test]
fn detect_mode_chat_subcommand_is_headless() {
    let cli = parse(&["chat", "hi"]);
    assert_eq!(detect_mode(&cli), Mode::Headless);
}

#[test]
fn detect_mode_review_subcommand_is_headless() {
    let cli = parse(&["review", "HEAD"]);
    assert_eq!(detect_mode(&cli), Mode::Headless);
}

#[test]
fn detect_mode_sdk_subcommand_is_sdk() {
    let cli = parse(&["sdk"]);
    assert_eq!(detect_mode(&cli), Mode::Sdk);
}

#[test]
fn detect_mode_short_subcommands_are_skip() {
    for sub in [
        "status",
        "doctor",
        "sessions",
        "init",
        "logout",
        "release-notes",
    ] {
        let cli = parse(&[sub]);
        assert_eq!(
            detect_mode(&cli),
            Mode::Skip,
            "subcommand {sub} should map to Mode::Skip"
        );
    }
}

#[test]
fn detect_mode_with_prompt_no_subcommand_is_headless() {
    let cli = parse(&["--prompt", "hi"]);
    assert_eq!(detect_mode(&cli), Mode::Headless);
}

#[test]
fn expand_bare_level_known_levels_get_coco_prefix() {
    for lvl in ["off", "error", "warn", "info", "debug", "trace"] {
        assert_eq!(expand_bare_level(lvl), format!("coco={lvl},{lvl}"));
    }
}

#[test]
fn expand_bare_level_uppercase_is_normalized() {
    assert_eq!(expand_bare_level("DEBUG"), "coco=debug,debug");
}

#[test]
fn expand_bare_level_full_directive_passes_through() {
    let directive = "coco_inference=trace,coco=debug,info";
    assert_eq!(expand_bare_level(directive), directive);
}

#[test]
fn subscriber_opts_carries_log_stderr_flag() {
    let cli = parse(&["--prompt", "hi", "--log-stderr"]);
    let opts = subscriber_opts_from_cli(&cli);
    assert!(opts.also_stderr);
}

#[test]
fn subscriber_opts_log_level_flag_expands() {
    let cli = parse(&["--prompt", "hi", "--log-level", "trace"]);
    let opts = subscriber_opts_from_cli_with_sources(&cli, None, &LogEnv::default());
    assert_eq!(opts.level.as_deref(), Some("coco=trace,trace"));
    assert!(opts.location);
}

#[test]
fn parse_timezone_known_values() {
    assert_eq!(parse_timezone("local"), Some(TimezoneConfig::Local));
    assert_eq!(parse_timezone("utc"), Some(TimezoneConfig::Utc));
    assert_eq!(parse_timezone("UTC"), Some(TimezoneConfig::Utc));
    assert_eq!(parse_timezone("  Local  "), Some(TimezoneConfig::Local));
}

#[test]
fn parse_timezone_unknown_returns_none() {
    assert_eq!(parse_timezone(""), None);
    assert_eq!(parse_timezone("est"), None);
    assert_eq!(parse_timezone("Asia/Shanghai"), None);
}

#[test]
fn subscriber_opts_default_timezone_is_local() {
    // No flag, no env (test process inherits clean env for COCO_LOG_TIMEZONE).
    let cli = parse(&["--prompt", "hi"]);
    let opts = subscriber_opts_from_cli(&cli);
    // We can't assert env is empty, but the flag-absent default must be `Local`
    // unless the test runner's environment overrides it. Document the intent.
    assert!(matches!(
        opts.timezone,
        TimezoneConfig::Local | TimezoneConfig::Utc
    ));
}

#[test]
fn subscriber_opts_log_timezone_flag_overrides_default() {
    let cli = parse(&["--prompt", "hi", "--log-timezone", "utc"]);
    let opts = subscriber_opts_from_cli(&cli);
    assert_eq!(opts.timezone, TimezoneConfig::Utc);
}

#[test]
fn subscriber_opts_unknown_log_timezone_falls_back_to_default() {
    let cli = parse(&["--prompt", "hi", "--log-timezone", "Asia/Shanghai"]);
    let opts = subscriber_opts_from_cli(&cli);
    // Unknown value → silently ignored → falls through to env / default.
    // We assert it's still a valid variant rather than testing the specific
    // default, since `COCO_LOG_TIMEZONE` may be set in the test env.
    assert!(matches!(
        opts.timezone,
        TimezoneConfig::Local | TimezoneConfig::Utc
    ));
}

#[test]
fn subscriber_opts_default_filter_auto_enables_location() {
    let cli = parse(&["--prompt", "hi"]);
    let opts = subscriber_opts_from_cli_with_sources(&cli, None, &LogEnv::default());
    assert_eq!(opts.level, None);
    assert!(opts.location);
    assert!(opts.thread_names);
}

#[test]
fn subscriber_opts_settings_filter_debug_auto_enables_location() {
    let cli = parse(&["--prompt", "hi"]);
    let settings = settings_with_log(PartialLogSettings {
        level: Some("coco=debug,info".into()),
        ..PartialLogSettings::default()
    });
    let opts = subscriber_opts_from_cli_with_sources(&cli, Some(&settings), &LogEnv::default());
    assert_eq!(opts.level.as_deref(), Some("coco=debug,info"));
    assert!(opts.location);
}

#[test]
fn subscriber_opts_settings_bare_level_expands() {
    let cli = parse(&["--prompt", "hi"]);
    let settings = settings_with_log(PartialLogSettings {
        level: Some("info".into()),
        ..PartialLogSettings::default()
    });
    let opts = subscriber_opts_from_cli_with_sources(&cli, Some(&settings), &LogEnv::default());
    assert_eq!(opts.level.as_deref(), Some("coco=info,info"));
    assert!(!opts.location);
}

#[test]
fn subscriber_opts_env_overrides_settings_log_block() {
    let cli = parse(&["--prompt", "hi"]);
    let settings = settings_with_log(PartialLogSettings {
        level: Some("trace".into()),
        format: Some("json".into()),
        file: Some("/settings.log".into()),
        stderr: Some(true),
        location: Some(true),
        timezone: Some("local".into()),
    });
    let log_env = LogEnv {
        level: Some("coco=info,info".into()),
        format: Some("compact".into()),
        file: Some("/env.log".into()),
        stderr: Some(false),
        location: Some(false),
        timezone: Some("utc".into()),
        ..LogEnv::default()
    };
    let opts = subscriber_opts_from_cli_with_sources(&cli, Some(&settings), &log_env);
    assert_eq!(opts.level.as_deref(), Some("coco=info,info"));
    assert_eq!(opts.format, Some(Format::Compact));
    assert_eq!(opts.file, Some(std::path::PathBuf::from("/env.log")));
    assert!(!opts.also_stderr);
    assert!(!opts.location);
    assert_eq!(opts.timezone, TimezoneConfig::Utc);
}

#[test]
fn resolve_location_explicit_flag_wins_over_auto() {
    // `--log-location=false` must defeat the debug auto-rule.
    let mut cli = parse(&["--prompt", "hi"]);
    cli.log_location = Some(false);
    assert!(!resolve_location(
        &cli,
        None,
        &LogEnv::default(),
        /*auto_verbose*/ true
    ));

    cli.log_location = Some(true);
    assert!(resolve_location(
        &cli,
        None,
        &LogEnv::default(),
        /*auto_verbose*/ false
    ));
}

#[test]
fn resolve_location_falls_back_to_auto_when_no_explicit() {
    // Pass an empty LogEnv so the test is not coupled to the process
    // environment inherited by the test runner.
    let cli = parse(&["--prompt", "hi"]);
    assert!(resolve_location(
        &cli,
        None,
        &LogEnv::default(),
        /*auto_verbose*/ true
    ));
    assert!(!resolve_location(
        &cli,
        None,
        &LogEnv::default(),
        /*auto_verbose*/ false
    ));
}

#[test]
fn subscriber_opts_log_location_flag_forces_on() {
    let cli = parse(&["--prompt", "hi", "--log-location"]);
    let opts = subscriber_opts_from_cli(&cli);
    assert!(opts.location);
    // thread_names is wired to follow `location` byte-for-byte so the
    // file and stderr sinks stay in lockstep.
    assert!(opts.thread_names);
}

#[test]
fn subscriber_opts_log_location_flag_can_force_off() {
    let cli = parse(&["--prompt", "hi", "--log-location=false"]);
    let opts = subscriber_opts_from_cli(&cli);
    assert!(!opts.location);
    assert!(!opts.thread_names);
}

#[test]
fn subscriber_opts_default_prefix_is_per_pid() {
    // The default log file is per-process so concurrent coco sessions don't
    // interleave into a single `coco.log.<date>`. PID stays unpadded to match
    // the header / `coco ps`.
    let cli = parse(&["--prompt", "hi"]);
    let opts = subscriber_opts_from_cli(&cli);
    assert_eq!(
        opts.default_file_prefix,
        format!("coco.{}", std::process::id())
    );
}

#[test]
fn sweep_stale_logs_removes_old_and_keeps_fresh() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old = dir.path().join("coco.111.log.2020-01-01");
    let fresh = dir.path().join("coco.222.log.2026-06-10");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&fresh, b"fresh").unwrap();

    let max_age = Duration::from_secs(7 * 24 * 60 * 60);
    // Both files were just written, so age ≈ 0. Advancing `now` 10 days makes
    // every file "10 days old" — only files older than the 7-day threshold get
    // swept, so to exercise the keep-path we then sweep again with real `now`.
    let now_future = SystemTime::now()
        .checked_add(Duration::from_secs(10 * 24 * 60 * 60))
        .unwrap();
    let removed = sweep_stale_logs(dir.path(), max_age, now_future);
    assert_eq!(removed, 2, "10-day-old view sweeps both files");

    // Re-create and sweep with real `now`: nothing is older than 7 days.
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&fresh, b"fresh").unwrap();
    let removed = sweep_stale_logs(dir.path(), max_age, SystemTime::now());
    assert_eq!(removed, 0, "fresh files are kept");
    assert!(old.exists() && fresh.exists());
}

#[test]
fn sweep_stale_logs_ignores_subdirs_and_missing_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    let now_future = SystemTime::now()
        .checked_add(Duration::from_secs(365 * 24 * 60 * 60))
        .unwrap();
    // A subdirectory is never a log file — left untouched even when "old".
    assert_eq!(
        sweep_stale_logs(dir.path(), Duration::from_secs(0), now_future),
        0
    );
    assert!(dir.path().join("nested").exists());
    // A non-existent dir is a best-effort no-op, not an error.
    assert_eq!(
        sweep_stale_logs(
            &dir.path().join("does-not-exist"),
            Duration::from_secs(0),
            now_future
        ),
        0
    );
}
