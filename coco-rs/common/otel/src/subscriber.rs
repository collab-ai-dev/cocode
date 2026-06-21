//! Centralized tracing subscriber bootstrap for coco-rs binaries.
//!
//! `tracing` is a no-op without a registered subscriber. Every
//! `info!` / `debug!` / `warn!` / `error!` call elsewhere in the
//! workspace silently does nothing until this module installs one.
//!
//! There must be exactly one install per process — call
//! [`init_subscriber`] from `main()` after parsing CLI args, and
//! retain the returned [`SubscriberHandle`] until shutdown so the
//! non-blocking file appender can flush on drop.
//!
//! Library and test code MUST NOT call [`init_subscriber`]. Tests that
//! need to assert on log output use [`init_for_tests`], which is
//! `OnceLock`-guarded so parallel tests can't double-init.
//!
//! ## Sink defaults per mode
//!
//! | Mode      | File sink | Stderr sink |
//! |-----------|-----------|-------------|
//! | `Tui`     | on        | off (opt-in via `also_stderr`) |
//! | `Sdk`     | on        | off (opt-in via `also_stderr`; stdout is NDJSON) |
//! | `Headless`| on        | on |
//! | `Skip`    | —         | —  (no subscriber installed) |
//!
//! The TUI / SDK defaults exist because both modes own stdout for
//! protocol or screen output; logs would corrupt them.

use std::error::Error;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use coco_utils_common::ConfigurableTimer;
use tracing::Level;
use tracing::Metadata;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;

/// Re-exported so callers (notably `app/cli/src/tracing_init.rs`) can
/// build `SubscriberOpts` without taking a direct dep on
/// `coco-utils-common`.
pub use coco_utils_common::TimezoneConfig;

/// Boxed error type — mirrors the existing `OtelProvider::from`
/// signature in `otel_provider.rs` so the subscriber stays consistent
/// with the rest of the crate.
pub type SubscriberError = Box<dyn Error + Send + Sync + 'static>;
pub type Result<T> = std::result::Result<T, SubscriberError>;

/// How the binary was invoked. Drives sink defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Interactive terminal UI — ratatui owns stdout/stderr.
    Tui,
    /// SDK NDJSON-over-stdio — stdout is the protocol channel.
    Sdk,
    /// Headless `--print` / piped stdout — stderr is free for logs.
    Headless,
    /// Short subcommand (status, doctor, …) — skip install entirely.
    Skip,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Sdk => "sdk",
            Self::Headless => "headless",
            Self::Skip => "skip",
        }
    }
}

/// Output format for the fmt layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Pretty,
    Compact,
    Json,
}

impl Format {
    /// Parse `pretty | compact | json`. Returns `None` for unrecognized
    /// input so callers can fall through to a default.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pretty" => Some(Self::Pretty),
            "compact" => Some(Self::Compact),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pretty => "pretty",
            Self::Compact => "compact",
            Self::Json => "json",
        }
    }
}

/// Pre-resolved subscriber options. The CLI layer merges its sources
/// (clap flags, `EnvKey::CocoLog*`, `RUST_LOG`, defaults) into one of
/// these and passes it in.
#[derive(Debug, Clone)]
pub struct SubscriberOpts {
    pub mode: Mode,
    /// Resolved `EnvFilter` directive. `None` falls through to
    /// [`DEFAULT_FILTER`].
    pub level: Option<String>,
    /// Resolved output format. `None` defers to the per-mode default
    /// chosen by [`default_format`].
    pub format: Option<Format>,
    /// Explicit log file path. `None` uses the rotating default
    /// `<default_log_dir>/<default_file_prefix>` (daily rotation).
    pub file: Option<PathBuf>,
    /// Force a stderr layer in addition to the file sink. Has no
    /// effect for [`Mode::Headless`] (which always writes to stderr).
    pub also_stderr: bool,
    /// Show source `filename:line` on each log event. Adds ~30–80 bytes
    /// per line. The CLI layer auto-enables this when the resolved
    /// filter enables DEBUG or TRACE events for coco targets.
    pub location: bool,
    /// Show thread name on each log event. Mostly cosmetic for the
    /// async hot path (worker threads cycle), but useful for panics
    /// and blocking-pool work. Wired to the same auto-rule as
    /// [`Self::location`].
    pub thread_names: bool,
    /// Directory used for the rotating-default log file.
    pub default_log_dir: PathBuf,
    /// Prefix used for the rotating-default log file. Daily rotation
    /// appends `.YYYY-MM-DD`.
    pub default_file_prefix: String,
    /// Timezone for log timestamps. Both the file and stderr fmt
    /// layers honor this via [`ConfigurableTimer`]. Defaults to
    /// [`TimezoneConfig::Local`].
    pub timezone: TimezoneConfig,
}

/// Handle returned by [`init_subscriber`]. Drop on shutdown to flush
/// the non-blocking file appender.
pub struct SubscriberHandle {
    _file_guard: Option<WorkerGuard>,
    /// Path to the rotating log file when one was opened. `None` if
    /// the file sink was disabled.
    pub log_path: Option<PathBuf>,
    /// Resolved filter directive in effect.
    pub effective_filter: String,
    /// Resolved format in effect.
    pub effective_format: Format,
}

/// Default filter directive when nothing else specifies one. Verbose
/// at `debug` for every `coco_*` crate (this is a dev-phase build),
/// `info` for everything else (third-party / tokio / hyper).
pub const DEFAULT_FILTER: &str = "coco=debug,info";

/// Returns true when a static tracing filter enables DEBUG-level events
/// for at least one `coco*` target.
///
/// This intentionally parses through `tracing-subscriber`'s `Targets`
/// filter so target-prefix and specificity behavior matches the fmt
/// layers. Dynamic span/field directives are ignored by this helper.
pub fn filter_enables_coco_debug(filter_directive: &str) -> bool {
    let Ok(targets) = filter_directive.parse::<Targets>() else {
        return false;
    };
    if targets.would_enable("coco", &Level::DEBUG) {
        return true;
    }
    targets.iter().any(|(target, _)| {
        target.starts_with("coco") && targets.would_enable(target, &Level::DEBUG)
    })
}

/// Build and register the global tracing subscriber. Safe to call
/// exactly once per process. Subsequent calls return an error from
/// the underlying `try_init`.
pub fn init_subscriber(mut opts: SubscriberOpts) -> Result<Option<SubscriberHandle>> {
    if matches!(opts.mode, Mode::Skip) {
        return Ok(None);
    }

    // `Option::take()` replaces the field with `None` so the rest of
    // `opts` stays whole and can still be borrowed below for path /
    // format resolution. A direct `unwrap_or_else` would partially
    // move `opts.level` (String is not `Copy`).
    let filter_directive = opts
        .level
        .take()
        .unwrap_or_else(|| DEFAULT_FILTER.to_string());
    // Validate the directive once up-front so the error path is
    // handled before we do filesystem I/O for the file appender.
    EnvFilter::try_new(&filter_directive).map_err(|e| -> SubscriberError {
        format!("invalid tracing filter {filter_directive:?}: {e}").into()
    })?;

    let format = opts.format.unwrap_or_else(|| default_format(opts.mode));
    let layout = LayoutOpts {
        location: opts.location,
        thread_names: opts.thread_names,
    };

    let (file_layer, file_guard, log_path) = build_file_sink(&opts, format, layout)?;

    let want_stderr = match opts.mode {
        Mode::Headless => true,
        Mode::Tui | Mode::Sdk => opts.also_stderr,
        Mode::Skip => unreachable!("Skip handled above"),
    };
    let stderr_layer = want_stderr.then(|| build_stderr_layer(format, &opts.timezone, layout));

    // Per-layer filter (one EnvFilter per fmt layer) instead of a
    // global subscriber filter. This lets each `Box<dyn Layer<Registry>>`
    // stay S-monomorphic at `Registry`; chaining via repeated
    // `.with()` would otherwise require the boxed layers to be
    // `Layer<Layered<…>>`, which they aren't after `.boxed()` erases
    // the S parameter. Two filters are built from the same directive
    // (already validated above) so both layers see the same rules.
    let make_filter = || EnvFilter::new(&filter_directive);
    let file_layer = file_layer.with_filter(make_filter()).boxed();
    let stderr_layer = stderr_layer.map(|l| l.with_filter(make_filter()).boxed());

    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync + 'static>> = Vec::new();
    layers.push(file_layer);
    if let Some(s) = stderr_layer {
        layers.push(s);
    }

    Registry::default()
        .with(layers)
        .try_init()
        .map_err(|e| -> SubscriberError {
            format!("tracing subscriber install failed: {e}").into()
        })?;

    Ok(Some(SubscriberHandle {
        _file_guard: file_guard,
        log_path,
        effective_filter: filter_directive,
        effective_format: format,
    }))
}

/// Default fmt format per mode. JSON for SDK so log files parse
/// machine-readably alongside the NDJSON protocol; compact for
/// terminal output (TUI/Headless) so lines stay readable.
pub fn default_format(mode: Mode) -> Format {
    match mode {
        Mode::Sdk => Format::Json,
        Mode::Tui | Mode::Headless => Format::Compact,
        Mode::Skip => Format::Pretty,
    }
}

/// Resolve the effective log file path from options. Pure — does no
/// I/O — so tests can verify path conventions without touching disk.
pub fn resolve_log_path(opts: &SubscriberOpts) -> PathBuf {
    opts.file.clone().unwrap_or_else(|| {
        opts.default_log_dir
            .join(format!("{}.log", opts.default_file_prefix))
    })
}

/// Boxed layer trait object used for both file and stderr sinks. The
/// monomorphic `Registry` parameter keeps every layer composable
/// without lifting up to `Layered<…>`.
type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

/// Per-layer layout toggles. Both sinks share the same values so the
/// file and stderr outputs stay in lockstep.
#[derive(Debug, Clone, Copy)]
struct LayoutOpts {
    location: bool,
    thread_names: bool,
}

fn build_file_sink(
    opts: &SubscriberOpts,
    format: Format,
    layout: LayoutOpts,
) -> Result<(BoxedLayer, Option<WorkerGuard>, Option<PathBuf>)> {
    let log_path = resolve_log_path(opts);
    let dir = log_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| -> SubscriberError {
        format!("creating log directory {}: {e}", dir.display()).into()
    })?;

    let prefix = log_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("coco.log")
        .to_string();
    let appender = rolling::daily(&dir, &prefix);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let layer = boxed_fmt_layer(format, writer, /*ansi*/ false, &opts.timezone, layout);
    Ok((layer, Some(guard), Some(log_path)))
}

fn build_stderr_layer(
    format: Format,
    timezone: &TimezoneConfig,
    layout: LayoutOpts,
) -> Box<dyn Layer<Registry> + Send + Sync + 'static> {
    boxed_fmt_layer(format, io::stderr, /*ansi*/ true, timezone, layout)
}

fn boxed_fmt_layer<W>(
    format: Format,
    writer: W,
    ansi: bool,
    timezone: &TimezoneConfig,
    layout: LayoutOpts,
) -> Box<dyn Layer<Registry> + Send + Sync + 'static>
where
    W: for<'a> fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    let writer = SourceFileBasenameWriter::new(writer, layout.location);
    let base = fmt::layer()
        .with_writer(writer)
        .with_ansi(ansi)
        .with_target(true)
        .with_file(layout.location)
        .with_line_number(layout.location)
        .with_thread_names(layout.thread_names)
        .with_timer(ConfigurableTimer::new(timezone.clone()));
    // `.boxed()` (from `Layer`) erases the per-format concrete type so
    // all three arms unify on `Box<dyn Layer<Registry>>`.
    match format {
        Format::Pretty => base.pretty().boxed(),
        Format::Compact => base.compact().boxed(),
        Format::Json => base.json().boxed(),
    }
}

struct SourceFileBasenameWriter<W> {
    inner: W,
    enabled: bool,
}

impl<W> SourceFileBasenameWriter<W> {
    fn new(inner: W, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

impl<'a, W> fmt::MakeWriter<'a> for SourceFileBasenameWriter<W>
where
    W: fmt::MakeWriter<'a>,
{
    type Writer = SourceFileBasenameEventWriter<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        SourceFileBasenameEventWriter::new(self.inner.make_writer(), None)
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        let source_file = self
            .enabled
            .then(|| meta.file())
            .flatten()
            .and_then(SourceFileLocation::new);
        SourceFileBasenameEventWriter::new(self.inner.make_writer_for(meta), source_file)
    }
}

struct SourceFileLocation {
    path: String,
    basename: String,
}

impl SourceFileLocation {
    fn new(source_file: &str) -> Option<Self> {
        let basename = source_file_basename(source_file);
        (basename != source_file).then(|| Self {
            path: source_file.to_string(),
            basename: basename.to_string(),
        })
    }
}

struct SourceFileBasenameEventWriter<W> {
    inner: W,
    source_file: Option<SourceFileLocation>,
}

impl<W> SourceFileBasenameEventWriter<W> {
    fn new(inner: W, source_file: Option<SourceFileLocation>) -> Self {
        Self { inner, source_file }
    }
}

impl<W> Write for SourceFileBasenameEventWriter<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(source_file) = &self.source_file else {
            return self.inner.write(buf);
        };

        let rewritten = replace_source_file(
            buf,
            source_file.path.as_bytes(),
            source_file.basename.as_bytes(),
        );
        self.inner.write_all(&rewritten)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn source_file_basename(source_file: &str) -> &str {
    source_file
        .rsplit(['/', '\\'])
        .next()
        .filter(|basename| !basename.is_empty())
        .unwrap_or(source_file)
}

fn replace_source_file(buf: &[u8], source_file: &[u8], basename: &[u8]) -> Vec<u8> {
    if source_file.is_empty() {
        return buf.to_vec();
    }

    let mut rewritten = Vec::with_capacity(buf.len());
    let mut remaining = buf;
    while let Some(index) = remaining
        .windows(source_file.len())
        .position(|window| window == source_file)
    {
        rewritten.extend_from_slice(&remaining[..index]);
        rewritten.extend_from_slice(basename);
        remaining = &remaining[index + source_file.len()..];
    }
    rewritten.extend_from_slice(remaining);
    rewritten
}

static TEST_SUBSCRIBER_INIT: OnceLock<()> = OnceLock::new();

/// Install a stderr-only subscriber for tests that want to assert on
/// log output. Idempotent — guarded by [`OnceLock`] so parallel tests
/// can't double-init the global subscriber.
///
/// Production / binary code MUST NOT call this — use
/// [`init_subscriber`] from `main()`.
pub fn init_for_tests() {
    TEST_SUBSCRIBER_INIT.get_or_init(|| {
        let env_filter =
            EnvFilter::try_new("coco=debug,debug").unwrap_or_else(|_| EnvFilter::new("debug"));
        let layout = LayoutOpts {
            location: false,
            thread_names: false,
        };
        let layer = build_stderr_layer(Format::Pretty, &TimezoneConfig::default(), layout)
            .with_filter(env_filter)
            .boxed();
        let _ = Registry::default().with(layer).try_init();
    });
}

#[cfg(test)]
#[path = "subscriber.test.rs"]
mod tests;
