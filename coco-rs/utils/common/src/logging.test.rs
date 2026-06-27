use super::*;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

#[test]
fn test_configurable_timer_local() {
    let timer = ConfigurableTimer::new(TimezoneConfig::Local);
    let mut buf = String::new();
    let mut writer = Writer::new(&mut buf);
    let _ = timer.format_time(&mut writer);
}

#[test]
fn test_configurable_timer_utc() {
    let timer = ConfigurableTimer::new(TimezoneConfig::Utc);
    let mut buf = String::new();
    let mut writer = Writer::new(&mut buf);
    let _ = timer.format_time(&mut writer);
}

#[test]
fn test_build_env_filter_with_default() {
    let logging = LoggingConfig::default();
    let filter = build_env_filter(&logging, "error");
    let _ = format!("{filter:?}");
}

#[test]
fn test_build_env_filter_with_modules() {
    let logging = LoggingConfig {
        location: false,
        target: false,
        timezone: TimezoneConfig::Local,
        level: "info".to_string(),
        modules: vec![
            "codex_core=debug".to_string(),
            "codex_tui=trace".to_string(),
        ],
    };
    let filter = build_env_filter(&logging, "error");
    let filter_str = format!("{filter:?}");
    assert!(filter_str.contains("codex_core") || filter_str.contains("debug"));
}
