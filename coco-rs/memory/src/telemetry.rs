//! Memory subsystem telemetry events.
//!
//! Each variant maps to one `tengu_*` event. Emission goes through
//! [`MemoryTelemetryEmitter`] so call sites stay free of OTel imports.

/// Memory event taxonomy.
#[derive(Debug, Clone)]
pub enum MemoryEvent {
    /// Memory directory loaded into the system prompt.
    MemdirLoaded {
        line_count: i64,
        byte_count: i64,
        was_truncated: bool,
        was_byte_truncated: bool,
        has_team: bool,
    },

    /// Memory subsystem is gated off.
    MemdirDisabled { reason: DisableReason },

    /// Mounted team promptIndex fetch outcome. Mirrors upstream's
    /// `tengu_feature_ok/sad` events with
    /// `feature_name=memory_prompt_index`.
    MemoryPromptIndex {
        mount: String,
        prompt_index: String,
        outcome: MemoryPromptIndexOutcome,
    },

    /// Extraction agent ran a tool that wasn't allow-listed.
    ExtractionToolDenied { tool_name: String },

    /// Background extraction skipped because the main agent already
    /// wrote memory files this turn.
    ExtractionSkippedDirectWrite { message_count: i32 },

    /// A new extraction request arrived while one was in-flight; the
    /// service stashed the latest context for a single trailing run.
    ExtractionCoalesced,

    /// Background extraction completed.
    ExtractionCompleted {
        turn_count: i32,
        input_tokens: i64,
        output_tokens: i64,
        /// `cache_read_input_tokens` — input tokens served from
        /// the prompt cache. Higher = better forked-agent hit-rate.
        cache_read_tokens: i64,
        /// `cache_creation_input_tokens` — input tokens written
        /// into the prompt cache. Should be small per-turn after
        /// the first one.
        cache_creation_tokens: i64,
        /// `files_written` — count after MEMORY.md is filtered
        /// out. The index file is mechanical; only topic-file writes
        /// count as "saved".
        files_written: i32,
        duration_ms: i64,
    },

    /// Background extraction subagent failed (turn budget exhausted,
    /// permission denial cascade, runner error, etc).
    ExtractionError { duration_ms: i64 },

    /// `/extract` (or equivalent slash command) forced an extraction
    /// bypassing throttle and direct-write gates. Lets dashboards
    /// split auto vs manual cadence.
    ExtractionManual,

    /// Auto-dream consolidation fired.
    AutoDreamFired {
        hours_since_last: i64,
        sessions_since_last: i32,
        team_memory_enabled: bool,
    },

    /// Auto-dream skipped at an upstream-observed telemetry branch.
    AutoDreamSkipped {
        reason: AutoDreamSkipReason,
        session_count: Option<i32>,
        min_required: Option<i32>,
    },

    /// Auto-dream consolidation completed.
    AutoDreamCompleted {
        sessions_reviewed: i32,
        /// `files_touched_count` field on `tengu_auto_dream_completed`.
        files_touched_count: i32,
        /// `team_memory_enabled` field on `tengu_auto_dream_completed`.
        team_memory_enabled: bool,
        /// `daily_logs_found` field on `tengu_auto_dream_completed`.
        daily_logs_found: i32,
        /// `cache_read` field on `tengu_auto_dream_completed`.
        cache_read_tokens: i64,
        /// `cache_created` field on `tengu_auto_dream_completed`.
        cache_creation_tokens: i64,
        /// `output` (output tokens) field.
        output_tokens: i64,
        duration_ms: i64,
    },

    /// Auto-dream consolidation subagent failed. Lock mtime is rolled
    /// back so the next time-gate window doesn't restart at "now".
    AutoDreamFailed {
        phase: AutoDreamFailurePhase,
        error_class: String,
    },

    /// `/dream` forced a consolidation bypassing the three gates. Lock
    /// is still acquired (concurrency-safe) and the lock mtime is
    /// rolled back on completion so the manual run doesn't perturb
    /// the auto cadence.
    AutoDreamManual,

    /// Session-memory extraction fired.
    SessionMemoryExtracted {
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
        duration_ms: i64,
    },

    /// Session-memory subsystem registered its post-sampling hook
    /// at session bootstrap.
    SessionMemoryInit { auto_compact_enabled: bool },

    /// Session-memory file was just read into context (typically
    /// from the setup pass before the forked update agent runs).
    SessionMemoryFileRead { content_length: i64 },

    /// Session-memory content loaded for compaction or other
    /// downstream consumers.
    SessionMemoryLoaded { content_length: i64 },

    /// `/summary` command forced a session-memory update bypassing
    /// the threshold gates.
    SessionMemoryManualExtraction,

    /// chmod 0o700/0o600 on a session-memory path failed. Flagged
    /// because the file body is potentially sensitive (conversation
    /// summary) and a failed chmod means it may be group/world
    /// readable on multi-user hosts. Rust's chmod path can race on
    /// platforms where setting permissions atomically isn't always
    /// available.
    SessionMemoryPermsFailed { path: String },

    /// KAIROS daily-log midnight rollover detected. The session
    /// crossed midnight local time; the engine receives the
    /// `Some(yesterday)` rollover signal so it can act on the date
    /// flip. We mirror the rollover *signal* only.
    KairosRollover {
        /// Day that just ended (`%Y-%m-%d`).
        yesterday: String,
        /// New active day (`%Y-%m-%d`).
        today: String,
    },
}

/// Outcome label for a mounted team promptIndex fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPromptIndexOutcome {
    Ok,
    Error,
    Timeout,
    UnsafePath,
}

impl MemoryPromptIndexOutcome {
    pub fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Ok => None,
            Self::Error => Some("error"),
            Self::Timeout => Some("timeout"),
            Self::UnsafePath => Some("unsafe_path"),
        }
    }
}

/// Reason labels used by `tengu_auto_dream_skipped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoDreamSkipReason {
    Sessions,
    Lock,
}

impl AutoDreamSkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Lock => "lock",
        }
    }
}

/// Phase labels used by `tengu_auto_dream_failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoDreamFailurePhase {
    Fork,
    Completion,
}

impl AutoDreamFailurePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fork => "fork",
            Self::Completion => "completion",
        }
    }
}

/// Reason auto-memory was disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisableReason {
    EnvVar,
    Settings,
    BareMode,
    RemoteMode,
    FeatureGate,
}

impl DisableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvVar => "env_var",
            Self::Settings => "settings",
            Self::BareMode => "bare_mode",
            Self::RemoteMode => "remote_mode",
            Self::FeatureGate => "feature_gate",
        }
    }
}

/// Trait the memory crate uses to emit events. Implemented by
/// `coco-otel`-backed adapters; tests use [`NoopEmitter`].
pub trait MemoryTelemetryEmitter: Send + Sync {
    fn emit(&self, event: MemoryEvent);
}

/// Default emitter — drops events on the floor.
#[derive(Debug, Default)]
pub struct NoopEmitter;

impl MemoryTelemetryEmitter for NoopEmitter {
    fn emit(&self, _event: MemoryEvent) {}
}

/// Lightweight emitter that fans every [`MemoryEvent`] into the global
/// `tracing` subscriber. This is the production-aligned path used by
/// `coco_otel::events::emit_*` — once `init_subscriber` is installed
/// from the binary, structured events flow through the configured OTel
/// exporters without any further wiring on the memory crate's side.
///
/// Cheap (no allocations beyond what the structured-field machinery
/// requires) and dependency-free — no need to construct an
/// [`coco_otel::OtelManager`] just to hand the crate an emitter. The
/// payload field names match the `tengu_*` event payload keys, so
/// dashboards keyed off those names keep working byte-for-byte.
#[derive(Debug, Default)]
pub struct TracingEmitter;

impl TracingEmitter {
    pub fn new() -> Self {
        Self
    }
}

impl MemoryTelemetryEmitter for TracingEmitter {
    fn emit(&self, event: MemoryEvent) {
        match event {
            MemoryEvent::MemdirLoaded {
                line_count,
                byte_count,
                was_truncated,
                was_byte_truncated,
                has_team,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_memdir_loaded",
                line_count,
                byte_count,
                was_truncated,
                was_byte_truncated,
                has_team,
                "memdir loaded"
            ),
            MemoryEvent::MemdirDisabled { reason } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_memdir_disabled",
                reason = reason.as_str(),
                "memdir disabled"
            ),
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome: MemoryPromptIndexOutcome::Ok,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_feature_ok",
                feature_name = "memory_prompt_index",
                mount = %mount,
                promptIndex = %prompt_index,
                "mounted memory prompt index fetched"
            ),
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome,
            } => tracing::warn!(
                target: "coco_memory::telemetry",
                event_type = "tengu_feature_sad",
                feature_name = "memory_prompt_index",
                error_code = outcome.error_code().unwrap_or("error"),
                mount = %mount,
                promptIndex = %prompt_index,
                "mounted memory prompt index fetch failed"
            ),
            MemoryEvent::ExtractionToolDenied { tool_name } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_mem_tool_denied",
                tool = %tool_name,
                "auto-mem tool denied"
            ),
            MemoryEvent::ExtractionSkippedDirectWrite { message_count } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_extract_memories_skipped_direct_write",
                message_count,
                "extract skipped — model wrote memory directly"
            ),
            MemoryEvent::ExtractionCoalesced => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_extract_memories_coalesced",
                "extract coalesced — stashed for trailing run"
            ),
            MemoryEvent::ExtractionCompleted {
                turn_count,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                files_written,
                duration_ms,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_extract_memories_extraction",
                turn_count,
                input_tokens,
                output_tokens,
                cache_read_input_tokens = cache_read_tokens,
                cache_creation_input_tokens = cache_creation_tokens,
                files_written,
                duration_ms,
                "extract completed"
            ),
            MemoryEvent::ExtractionError { duration_ms } => tracing::warn!(
                target: "coco_memory::telemetry",
                event_type = "tengu_extract_memories_error",
                duration_ms,
                "extract failed"
            ),
            MemoryEvent::ExtractionManual => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_extract_memories_manual",
                "extract forced manually"
            ),
            MemoryEvent::AutoDreamFired {
                hours_since_last,
                sessions_since_last,
                team_memory_enabled,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_dream_fired",
                hours_since_last,
                sessions_since_last,
                team_memory_enabled,
                "auto-dream fired"
            ),
            MemoryEvent::AutoDreamSkipped {
                reason: AutoDreamSkipReason::Sessions,
                session_count,
                min_required,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_dream_skipped",
                reason = "sessions",
                session_count = session_count.unwrap_or_default(),
                min_required = min_required.unwrap_or_default(),
                "auto-dream skipped"
            ),
            MemoryEvent::AutoDreamSkipped {
                reason: AutoDreamSkipReason::Lock,
                session_count: _,
                min_required: _,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_dream_skipped",
                reason = "lock",
                "auto-dream skipped"
            ),
            MemoryEvent::AutoDreamCompleted {
                sessions_reviewed,
                files_touched_count,
                team_memory_enabled,
                daily_logs_found,
                cache_read_tokens,
                cache_creation_tokens,
                output_tokens,
                duration_ms,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_dream_completed",
                sessions_reviewed,
                files_touched_count,
                team_memory_enabled,
                daily_logs_found,
                cache_read = cache_read_tokens,
                cache_created = cache_creation_tokens,
                output = output_tokens,
                duration_ms,
                "auto-dream completed"
            ),
            MemoryEvent::AutoDreamFailed { phase, error_class } => tracing::warn!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_dream_failed",
                phase = phase.as_str(),
                error_class = %error_class,
                "auto-dream failed"
            ),
            MemoryEvent::AutoDreamManual => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_auto_dream_manual",
                "auto-dream forced manually"
            ),
            MemoryEvent::SessionMemoryExtracted {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                duration_ms,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_session_memory_extraction",
                input_tokens,
                output_tokens,
                cache_read_input_tokens = cache_read_tokens,
                cache_creation_input_tokens = cache_creation_tokens,
                duration_ms,
                "session-memory extracted"
            ),
            MemoryEvent::SessionMemoryInit {
                auto_compact_enabled,
            } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_session_memory_init",
                auto_compact_enabled,
                "session-memory init"
            ),
            MemoryEvent::SessionMemoryFileRead { content_length } => tracing::debug!(
                target: "coco_memory::telemetry",
                event_type = "tengu_session_memory_file_read",
                content_length,
                "session-memory file read"
            ),
            MemoryEvent::SessionMemoryLoaded { content_length } => tracing::debug!(
                target: "coco_memory::telemetry",
                event_type = "tengu_session_memory_loaded",
                content_length,
                "session-memory loaded"
            ),
            MemoryEvent::SessionMemoryManualExtraction => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_session_memory_manual_extraction",
                "session-memory forced manually"
            ),
            MemoryEvent::SessionMemoryPermsFailed { path } => tracing::warn!(
                target: "coco_memory::telemetry",
                event_type = "coco_session_memory_perms_failed",
                path = %path,
                "session-memory chmod failed"
            ),
            MemoryEvent::KairosRollover { yesterday, today } => tracing::info!(
                target: "coco_memory::telemetry",
                event_type = "tengu_kairos_rollover",
                yesterday = %yesterday,
                today = %today,
                "kairos rollover"
            ),
        }
    }
}

/// Adapter that maps [`MemoryEvent`] onto an [`coco_otel::OtelManager`].
///
/// Each `tengu_*` event lands as a counter; numeric payload fields
/// (token counts, durations, file counts) are emitted as histograms /
/// `record_duration` so dashboards can chart distribution. Tag values
/// preserve the event names so downstream pipelines that already
/// know `tengu_extract_memories_extraction` keep working.
#[derive(Clone)]
pub struct OtelEmitter {
    manager: std::sync::Arc<coco_otel::OtelManager>,
}

impl std::fmt::Debug for OtelEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtelEmitter").finish()
    }
}

impl OtelEmitter {
    pub fn new(manager: std::sync::Arc<coco_otel::OtelManager>) -> Self {
        Self { manager }
    }
}

impl MemoryTelemetryEmitter for OtelEmitter {
    fn emit(&self, event: MemoryEvent) {
        match event {
            MemoryEvent::MemdirLoaded {
                line_count,
                byte_count,
                was_truncated,
                was_byte_truncated,
                has_team,
            } => {
                let truncated = bool_str(was_truncated);
                let byte_truncated = bool_str(was_byte_truncated);
                let team = bool_str(has_team);
                self.manager.counter(
                    "tengu_memdir_loaded",
                    1,
                    &[
                        ("was_truncated", truncated),
                        ("was_byte_truncated", byte_truncated),
                        ("has_team", team),
                    ],
                );
                self.manager.histogram("memdir.line_count", line_count, &[]);
                self.manager.histogram("memdir.byte_count", byte_count, &[]);
            }
            MemoryEvent::MemdirDisabled { reason } => {
                self.manager
                    .counter("tengu_memdir_disabled", 1, &[("reason", reason.as_str())]);
            }
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome: MemoryPromptIndexOutcome::Ok,
            } => {
                self.manager.counter(
                    "tengu_feature_ok",
                    1,
                    &[
                        ("feature_name", "memory_prompt_index"),
                        ("mount", mount.as_str()),
                        ("promptIndex", prompt_index.as_str()),
                    ],
                );
            }
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome,
            } => {
                self.manager.counter(
                    "tengu_feature_sad",
                    1,
                    &[
                        ("feature_name", "memory_prompt_index"),
                        ("error_code", outcome.error_code().unwrap_or("error")),
                        ("mount", mount.as_str()),
                        ("promptIndex", prompt_index.as_str()),
                    ],
                );
            }
            MemoryEvent::ExtractionToolDenied { tool_name } => {
                self.manager.counter(
                    "tengu_auto_mem_tool_denied",
                    1,
                    &[("tool", tool_name.as_str())],
                );
            }
            MemoryEvent::ExtractionSkippedDirectWrite { message_count } => {
                self.manager
                    .counter("tengu_extract_memories_skipped_direct_write", 1, &[]);
                self.manager
                    .histogram("extract.message_count", message_count as i64, &[]);
            }
            MemoryEvent::ExtractionCoalesced => {
                self.manager
                    .counter("tengu_extract_memories_coalesced", 1, &[]);
            }
            MemoryEvent::ExtractionError { duration_ms } => {
                self.manager.counter("tengu_extract_memories_error", 1, &[]);
                self.manager.record_duration(
                    "extract.duration",
                    std::time::Duration::from_millis(duration_ms.max(0) as u64),
                    &[("outcome", "error")],
                );
            }
            MemoryEvent::ExtractionManual => {
                self.manager
                    .counter("tengu_extract_memories_manual", 1, &[]);
            }
            MemoryEvent::ExtractionCompleted {
                turn_count,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                files_written,
                duration_ms,
            } => {
                self.manager
                    .counter("tengu_extract_memories_extraction", 1, &[]);
                self.manager
                    .histogram("extract.turn_count", turn_count as i64, &[]);
                self.manager
                    .histogram("extract.input_tokens", input_tokens, &[]);
                self.manager
                    .histogram("extract.output_tokens", output_tokens, &[]);
                self.manager
                    .histogram("extract.cache_read_tokens", cache_read_tokens, &[]);
                self.manager
                    .histogram("extract.cache_creation_tokens", cache_creation_tokens, &[]);
                self.manager
                    .histogram("extract.files_written", files_written as i64, &[]);
                self.manager.record_duration(
                    "extract.duration",
                    std::time::Duration::from_millis(duration_ms.max(0) as u64),
                    &[],
                );
            }
            MemoryEvent::AutoDreamFired {
                hours_since_last,
                sessions_since_last,
                team_memory_enabled,
            } => {
                self.manager.counter(
                    "tengu_auto_dream_fired",
                    1,
                    &[("team_memory_enabled", bool_str(team_memory_enabled))],
                );
                self.manager
                    .histogram("dream.hours_since_last", hours_since_last, &[]);
                self.manager.histogram(
                    "dream.sessions_since_last",
                    sessions_since_last as i64,
                    &[],
                );
            }
            MemoryEvent::AutoDreamSkipped {
                reason: AutoDreamSkipReason::Sessions,
                session_count,
                min_required,
            } => {
                self.manager.counter(
                    "tengu_auto_dream_skipped",
                    1,
                    &[("reason", AutoDreamSkipReason::Sessions.as_str())],
                );
                if let Some(count) = session_count {
                    self.manager
                        .histogram("dream.skipped.session_count", count as i64, &[]);
                }
                if let Some(min) = min_required {
                    self.manager
                        .histogram("dream.skipped.min_required", min as i64, &[]);
                }
            }
            MemoryEvent::AutoDreamSkipped {
                reason: AutoDreamSkipReason::Lock,
                session_count: _,
                min_required: _,
            } => {
                self.manager.counter(
                    "tengu_auto_dream_skipped",
                    1,
                    &[("reason", AutoDreamSkipReason::Lock.as_str())],
                );
            }
            MemoryEvent::AutoDreamCompleted {
                sessions_reviewed,
                files_touched_count,
                team_memory_enabled,
                daily_logs_found,
                cache_read_tokens,
                cache_creation_tokens,
                output_tokens,
                duration_ms,
            } => {
                self.manager.counter(
                    "tengu_auto_dream_completed",
                    1,
                    &[("team_memory_enabled", bool_str(team_memory_enabled))],
                );
                self.manager
                    .histogram("dream.sessions_reviewed", sessions_reviewed as i64, &[]);
                self.manager.histogram(
                    "dream.files_touched_count",
                    files_touched_count as i64,
                    &[],
                );
                self.manager
                    .histogram("dream.daily_logs_found", daily_logs_found as i64, &[]);
                self.manager
                    .histogram("dream.cache_read_tokens", cache_read_tokens, &[]);
                self.manager
                    .histogram("dream.cache_creation_tokens", cache_creation_tokens, &[]);
                self.manager
                    .histogram("dream.output_tokens", output_tokens, &[]);
                self.manager.record_duration(
                    "dream.duration",
                    std::time::Duration::from_millis(duration_ms.max(0) as u64),
                    &[],
                );
            }
            MemoryEvent::AutoDreamFailed { phase, error_class } => {
                self.manager.counter(
                    "tengu_auto_dream_failed",
                    1,
                    &[
                        ("phase", phase.as_str()),
                        ("error_class", error_class.as_str()),
                    ],
                );
            }
            MemoryEvent::AutoDreamManual => {
                self.manager.counter("tengu_auto_dream_manual", 1, &[]);
            }
            MemoryEvent::SessionMemoryExtracted {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                duration_ms,
            } => {
                self.manager
                    .counter("tengu_session_memory_extraction", 1, &[]);
                self.manager
                    .histogram("session_memory.input_tokens", input_tokens, &[]);
                self.manager
                    .histogram("session_memory.output_tokens", output_tokens, &[]);
                self.manager
                    .histogram("session_memory.cache_read_tokens", cache_read_tokens, &[]);
                self.manager.histogram(
                    "session_memory.cache_creation_tokens",
                    cache_creation_tokens,
                    &[],
                );
                self.manager.record_duration(
                    "session_memory.duration",
                    std::time::Duration::from_millis(duration_ms.max(0) as u64),
                    &[],
                );
            }
            MemoryEvent::SessionMemoryInit {
                auto_compact_enabled,
            } => {
                self.manager.counter(
                    "tengu_session_memory_init",
                    1,
                    &[("auto_compact_enabled", bool_str(auto_compact_enabled))],
                );
            }
            MemoryEvent::SessionMemoryFileRead { content_length } => {
                self.manager
                    .counter("tengu_session_memory_file_read", 1, &[]);
                self.manager
                    .histogram("session_memory.file_read_length", content_length, &[]);
            }
            MemoryEvent::SessionMemoryLoaded { content_length } => {
                self.manager.counter("tengu_session_memory_loaded", 1, &[]);
                self.manager
                    .histogram("session_memory.loaded_length", content_length, &[]);
            }
            MemoryEvent::SessionMemoryManualExtraction => {
                self.manager
                    .counter("tengu_session_memory_manual_extraction", 1, &[]);
            }
            MemoryEvent::SessionMemoryPermsFailed { path: _ } => {
                self.manager
                    .counter("coco_session_memory_perms_failed", 1, &[]);
            }
            MemoryEvent::KairosRollover {
                yesterday: _,
                today: _,
            } => {
                self.manager.counter("tengu_kairos_rollover", 1, &[]);
            }
        }
    }
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}
