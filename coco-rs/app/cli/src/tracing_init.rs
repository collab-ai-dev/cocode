//! Tracing subscriber bootstrap for the `coco` binary.
//!
//! Single entry point: [`install`]. Called once from `main()` after
//! `Cli::parse()` so subscriber config can honor flags AND env. Returns
//! a [`SubscriberHandle`] the binary must keep alive for the process
//! lifetime — dropping it flushes the non-blocking file appender.
//!
//! Resolution priority for the filter directive:
//!   `--log-level` > `COCO_LOG` > `RUST_LOG` > `settings.log.level`
//!   > `coco_otel::subscriber::DEFAULT_FILTER`.
//!
//! `COCO_LOG_FORMAT`, `COCO_LOG_FILE`, `COCO_LOG_STDERR`,
//! `COCO_LOG_LOCATION`, `COCO_LOG_TIMEZONE` mirror their `--log-*` flag
//! counterparts at lower priority and higher priority than
//! `settings.log.*`.
//!
//! Auto-verbose: when the resolved filter enables DEBUG or TRACE for a
//! `coco*` target, location + thread-name layout default to on. Explicit
//! `--log-location`, `COCO_LOG_LOCATION`, or `settings.log.location`
//! values still win.

use std::io::IsTerminal;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Result;
use coco_config::EnvKey;
use coco_config::Settings;
use coco_config::env;
use coco_config::global_config;
use coco_otel::subscriber::DEFAULT_FILTER;
use coco_otel::subscriber::Format;
use coco_otel::subscriber::Mode;
use coco_otel::subscriber::SubscriberHandle;
use coco_otel::subscriber::SubscriberOpts;
use coco_otel::subscriber::TimezoneConfig;
use coco_otel::subscriber::filter_enables_coco_debug;
use coco_otel::subscriber::init_subscriber;

use crate::Cli;
use crate::Commands;

/// Install the global tracing subscriber. Returns `Ok(None)` for short
/// subcommands that exit before any heavy work — they get
/// [`Mode::Skip`] so their stdout stays clean.
pub fn install(cli: &Cli) -> Result<Option<SubscriberHandle>> {
    let mode = detect_mode(cli);
    let settings = if matches!(mode, Mode::Skip) {
        None
    } else {
        Some(load_settings_for_logging(cli)?)
    };
    let opts =
        subscriber_opts_from_cli_with_sources(cli, settings.as_ref(), &LogEnv::from_process());
    let mode = opts.mode;
    let location = opts.location;
    let handle = init_subscriber(opts).map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(h) = &handle {
        emit_ready_anchor(mode, h, location);
        if let Some(path) = &h.log_path {
            // Headless has no TUI header to surface the per-PID file, and the
            // anchor above only reaches stderr when the filter keeps `info`.
            // Print the path unconditionally so a piped/`--print` run can
            // always find its own log among concurrent sessions.
            if matches!(mode, Mode::Headless) {
                eprintln!("coco: logging to {}", path.display());
            }
            // Per-PID files (`coco.<pid>.log.<date>`) escape the daily
            // appender's own rotation — each process only knows its own
            // prefix — so sweep the dir by mtime to keep it bounded.
            if let Some(dir) = path.parent() {
                sweep_stale_logs(
                    dir,
                    Duration::from_secs(LOG_RETENTION_SECS),
                    SystemTime::now(),
                );
            }
        }
    }
    Ok(handle)
}

/// Age past which a log file is swept on startup. Per-PID log files
/// accumulate (one prefix per process), so a plain mtime sweep keeps
/// `<config_home>/logs/` bounded without any cross-process bookkeeping.
const LOG_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

/// Delete every regular file directly under `dir` whose mtime is older
/// than `max_age` relative to `now`. `logs/` is coco-owned (only log
/// files live there), so no filename matching is needed — this also
/// reaps legacy single-file `coco.log.<date>` logs. Best-effort:
/// unreadable entries and failed unlinks are skipped. Returns the count
/// removed (for tests). A still-running process keeps a fresh mtime on
/// its current-day file, so liveness need not be checked separately.
fn sweep_stale_logs(dir: &Path, max_age: Duration, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        // `duration_since` errors when `modified` is in the future (clock
        // skew); treat those as fresh and keep them.
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > max_age && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Build [`SubscriberOpts`] from CLI flags + `COCO_LOG*` env vars.
/// Pure assembly — does not register the global subscriber.
pub fn subscriber_opts_from_cli(cli: &Cli) -> SubscriberOpts {
    subscriber_opts_from_cli_with_sources(cli, None, &LogEnv::from_process())
}

fn subscriber_opts_from_cli_with_sources(
    cli: &Cli,
    settings: Option<&Settings>,
    log_env: &LogEnv,
) -> SubscriberOpts {
    let level = resolve_level(cli, settings, log_env);
    let auto_verbose = filter_enables_coco_debug(level.as_deref().unwrap_or(DEFAULT_FILTER));
    let location = resolve_location(cli, settings, log_env, auto_verbose);
    SubscriberOpts {
        mode: detect_mode(cli),
        level,
        format: resolve_format(cli, settings, log_env),
        file: resolve_file(cli, settings, log_env),
        also_stderr: resolve_stderr(cli, settings, log_env),
        location,
        thread_names: location,
        default_log_dir: global_config::config_home().join("logs"),
        // Per-PID prefix → `coco.<pid>.log.<date>`. Isolates each process's
        // log so concurrent coco sessions don't interleave into one file.
        // PID stays unpadded to match the header / `coco ps` / `std::process::id()`.
        default_file_prefix: format!("coco.{}", std::process::id()),
        timezone: resolve_timezone(cli, settings, log_env),
    }
}

fn load_settings_for_logging(cli: &Cli) -> Result<Settings> {
    let cwd = std::env::current_dir()?;
    let flag_settings = cli.settings.as_deref().map(Path::new);
    Ok(coco_config::settings::load_settings(&cwd, flag_settings)?.merged)
}

#[derive(Debug, Default)]
struct LogEnv {
    level: Option<String>,
    rust_log: Option<String>,
    format: Option<String>,
    file: Option<String>,
    stderr: Option<bool>,
    location: Option<bool>,
    timezone: Option<String>,
}

impl LogEnv {
    fn from_process() -> Self {
        Self {
            level: env::env_opt(EnvKey::CocoLog),
            rust_log: std::env::var("RUST_LOG").ok().filter(|s| !s.is_empty()),
            format: env::env_opt(EnvKey::CocoLogFormat),
            file: env::env_opt(EnvKey::CocoLogFile),
            stderr: env::env_truthy_opt(EnvKey::CocoLogStderr),
            location: env::env_truthy_opt(EnvKey::CocoLogLocation),
            timezone: env::env_opt(EnvKey::CocoLogTimezone),
        }
    }
}

/// Pick the run mode by inspecting the subcommand and stdin/stdout
/// terminal state. Same branching as `main()` so subscriber sinks
/// match the eventual run path.
pub fn detect_mode(cli: &Cli) -> Mode {
    if let Some(cmd) = &cli.command {
        return match cmd {
            Commands::Chat { .. } | Commands::Review { .. } => Mode::Headless,
            Commands::Sdk => Mode::Sdk,
            // Every other subcommand prints a short result and exits;
            // installing a subscriber would only add log noise.
            _ => Mode::Skip,
        };
    }

    let is_piped = !std::io::stdout().is_terminal();
    if cli.prompt.is_some() || is_piped {
        Mode::Headless
    } else {
        Mode::Tui
    }
}

fn resolve_level(cli: &Cli, settings: Option<&Settings>, log_env: &LogEnv) -> Option<String> {
    if let Some(raw) = cli.log_level.as_deref() {
        return Some(expand_bare_level(raw));
    }
    if let Some(directive) = log_env.level.clone() {
        return Some(directive);
    }
    // RUST_LOG is the tracing-ecosystem convention name; it is still
    // an environment override and therefore wins over settings.json.
    if let Some(directive) = log_env.rust_log.clone() {
        return Some(directive);
    }
    settings
        .and_then(|s| s.log.level.as_deref())
        .filter(|s| !s.trim().is_empty())
        .map(expand_bare_level)
}

fn resolve_format(cli: &Cli, settings: Option<&Settings>, log_env: &LogEnv) -> Option<Format> {
    cli.log_format
        .as_deref()
        .and_then(Format::parse)
        .or_else(|| log_env.format.as_deref().and_then(Format::parse))
        .or_else(|| {
            settings
                .and_then(|s| s.log.format.as_deref())
                .and_then(Format::parse)
        })
}

fn resolve_file(cli: &Cli, settings: Option<&Settings>, log_env: &LogEnv) -> Option<PathBuf> {
    cli.log_file
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| log_env.file.as_deref().map(PathBuf::from))
        .or_else(|| {
            settings
                .and_then(|s| s.log.file.as_deref())
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
        })
}

fn resolve_stderr(cli: &Cli, settings: Option<&Settings>, log_env: &LogEnv) -> bool {
    if cli.log_stderr {
        return true;
    }
    if let Some(v) = log_env.stderr {
        return v;
    }
    settings.and_then(|s| s.log.stderr).unwrap_or(false)
}

/// Tri-state resolution for the verbose-layout switch (location +
/// thread name). Explicit flag wins, then env, then settings, then
/// auto-rule.
fn resolve_location(
    cli: &Cli,
    settings: Option<&Settings>,
    log_env: &LogEnv,
    auto_verbose: bool,
) -> bool {
    if let Some(v) = cli.log_location {
        return v;
    }
    if let Some(v) = log_env.location {
        return v;
    }
    if let Some(v) = settings.and_then(|s| s.log.location) {
        return v;
    }
    auto_verbose
}

/// Resolve the timezone for log timestamps. `--log-timezone` wins over
/// `COCO_LOG_TIMEZONE`, which wins over `settings.log.timezone`; all
/// accept `local | utc` (case-insensitive). Unknown values fall through
/// to [`TimezoneConfig::default`] (`Local`).
fn resolve_timezone(cli: &Cli, settings: Option<&Settings>, log_env: &LogEnv) -> TimezoneConfig {
    cli.log_timezone
        .as_deref()
        .and_then(parse_timezone)
        .or_else(|| log_env.timezone.as_deref().and_then(parse_timezone))
        .or_else(|| {
            settings
                .and_then(|s| s.log.timezone.as_deref())
                .and_then(parse_timezone)
        })
        .unwrap_or_default()
}

fn parse_timezone(raw: &str) -> Option<TimezoneConfig> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "local" => Some(TimezoneConfig::Local),
        "utc" => Some(TimezoneConfig::Utc),
        _ => None,
    }
}

/// Bare levels (`debug`, `info`, …) expand to `coco=<lvl>,<lvl>` so
/// coco crates stay verbose without spamming third-party output.
/// Anything else is passed through as a full `EnvFilter` directive.
fn expand_bare_level(raw: &str) -> String {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "off" | "error" | "warn" | "info" | "debug" | "trace"
    ) {
        format!("coco={lower},{lower}")
    } else {
        raw.to_string()
    }
}

fn emit_ready_anchor(mode: Mode, handle: &SubscriberHandle, location: bool) {
    match handle.log_path.as_ref() {
        Some(path) => tracing::info!(
            target: "coco_cli::startup",
            coco_version = env!("CARGO_PKG_VERSION"),
            mode = mode.as_str(),
            log_filter = %handle.effective_filter,
            log_format = handle.effective_format.as_str(),
            log_file = %path.display(),
            log_location = location,
            "subscriber ready"
        ),
        None => tracing::info!(
            target: "coco_cli::startup",
            coco_version = env!("CARGO_PKG_VERSION"),
            mode = mode.as_str(),
            log_filter = %handle.effective_filter,
            log_format = handle.effective_format.as_str(),
            log_location = location,
            "subscriber ready (file sink disabled)"
        ),
    }
}

#[cfg(test)]
#[path = "tracing_init.test.rs"]
mod tests;
