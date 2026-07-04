use std::time::Duration;

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;

use crate::display_settings::TuiPerformanceConfig;
use crate::state::AppState;
use crate::transcript::cells::CellKind;

pub(crate) const TARGET: &str = "tui::perf::frame";
pub(crate) const MEM_TARGET: &str = "tui::perf::mem";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameInputStats {
    pub core_events: u64,
    pub stream_text_deltas: u64,
    pub stream_thinking_deltas: u64,
    pub terminal_inputs: u64,
    pub ticks: u64,
    pub settings_reloads: u64,
}

impl FrameInputStats {
    pub(crate) fn record_core_event(&mut self, event: &CoreEvent) {
        self.core_events += 1;
        match event {
            CoreEvent::Stream(AgentStreamEvent::TextDelta { .. }) => {
                self.stream_text_deltas += 1;
            }
            CoreEvent::Stream(AgentStreamEvent::ThinkingDelta { .. }) => {
                self.stream_thinking_deltas += 1;
            }
            _ => {}
        }
    }
}

pub(crate) fn should_log_frame(
    config: TuiPerformanceConfig,
    frame_index: u64,
    duration: Duration,
) -> bool {
    if !config.enabled {
        return false;
    }
    sampled(config, frame_index) || duration.as_millis() >= u128::from(config.slow_frame_ms)
}

pub(crate) fn should_log_stage(
    config: TuiPerformanceConfig,
    frame_index: u64,
    duration: Duration,
) -> bool {
    if !config.enabled {
        return false;
    }
    sampled(config, frame_index) || duration.as_micros() >= u128::from(config.slow_stage_us)
}

pub(crate) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

pub(crate) fn duration_us(duration: Duration) -> u128 {
    duration.as_micros()
}

fn sampled(config: TuiPerformanceConfig, frame_index: u64) -> bool {
    config.sample_every_n_frames != 0 && frame_index.is_multiple_of(config.sample_every_n_frames)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemorySampleKind {
    Lifecycle,
    Periodic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryPhase {
    Startup,
    FirstDraw,
    Periodic,
    TurnStarted,
    EngineReturned,
    HistoryReplaced,
    TurnEnded,
    ContextCleared,
    MessageTruncated,
    SessionReset,
}

impl MemoryPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::FirstDraw => "first_draw",
            Self::Periodic => "periodic",
            Self::TurnStarted => "turn_started",
            Self::EngineReturned => "engine_returned",
            Self::HistoryReplaced => "history_replaced",
            Self::TurnEnded => "turn_ended",
            Self::ContextCleared => "context_cleared",
            Self::MessageTruncated => "message_truncated",
            Self::SessionReset => "session_reset",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessMemorySample {
    pub(crate) rss_bytes: u64,
    pub(crate) vsz_bytes: u64,
    pub(crate) sample_ms: u128,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RetainedMemoryStats {
    pub(crate) message_history_payload_bytes: usize,
    pub(crate) transcript_cell_text_bytes: usize,
    pub(crate) tool_execution_bytes: usize,
    pub(crate) reasoning_metadata_bytes: usize,
    pub(crate) subagent_bytes: usize,
    pub(crate) last_markdown_bytes: usize,
    pub(crate) markdown_memo_cache_bytes: usize,
    pub(crate) history_replay_cache_bytes: usize,
}

impl RetainedMemoryStats {
    pub(crate) fn retained_total_bytes(self) -> usize {
        self.message_history_payload_bytes
            .saturating_add(self.transcript_cell_text_bytes)
            .saturating_add(self.tool_execution_bytes)
            .saturating_add(self.reasoning_metadata_bytes)
            .saturating_add(self.subagent_bytes)
            .saturating_add(self.last_markdown_bytes)
            .saturating_add(self.markdown_memo_cache_bytes)
            .saturating_add(self.history_replay_cache_bytes)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryPerfTracker {
    last_logged_rss_bytes: Option<u64>,
}

impl MemoryPerfTracker {
    pub(crate) fn enabled(config: TuiPerformanceConfig) -> bool {
        config.memory_enabled
    }

    pub(crate) fn periodic_enabled(config: TuiPerformanceConfig) -> bool {
        Self::enabled(config) && config.memory_sample_interval_secs != 0
    }

    pub(crate) fn periodic_interval(config: TuiPerformanceConfig) -> Duration {
        Duration::from_secs(config.memory_sample_interval_secs.max(1))
    }

    pub(crate) fn maybe_log(
        &mut self,
        config: TuiPerformanceConfig,
        phase: MemoryPhase,
        sample_kind: MemorySampleKind,
        retained: RetainedMemoryStats,
    ) {
        if !Self::enabled(config) {
            return;
        }

        let Ok(sample) = sample_current_process_memory() else {
            return;
        };

        let previous_rss = self.last_logged_rss_bytes;
        let rss_delta_bytes = previous_rss
            .map(|previous| sample.rss_bytes as i128 - previous as i128)
            .unwrap_or(0);
        let threshold_hit = config.memory_delta_threshold_bytes != 0
            && previous_rss.is_some()
            && rss_delta_bytes.unsigned_abs() >= u128::from(config.memory_delta_threshold_bytes);
        let should_log = matches!(
            sample_kind,
            MemorySampleKind::Lifecycle | MemorySampleKind::Periodic
        ) || threshold_hit;
        if !should_log {
            return;
        }

        let trigger = trigger_label(sample_kind, threshold_hit);
        tracing::debug!(
            target: MEM_TARGET,
            trigger,
            phase = phase.as_str(),
            rss_bytes = sample.rss_bytes,
            vsz_bytes = sample.vsz_bytes,
            rss_delta_bytes,
            sample_ms = sample.sample_ms,
            source = "macos_ps",
            message_history_payload_bytes = retained.message_history_payload_bytes,
            transcript_cell_text_bytes = retained.transcript_cell_text_bytes,
            tool_execution_bytes = retained.tool_execution_bytes,
            reasoning_metadata_bytes = retained.reasoning_metadata_bytes,
            subagent_bytes = retained.subagent_bytes,
            last_markdown_bytes = retained.last_markdown_bytes,
            markdown_memo_cache_bytes = retained.markdown_memo_cache_bytes,
            history_replay_cache_bytes = retained.history_replay_cache_bytes,
            retained_total_bytes = retained.retained_total_bytes(),
            "tui memory sample",
        );
        self.last_logged_rss_bytes = Some(sample.rss_bytes);
    }
}

fn trigger_label(sample_kind: MemorySampleKind, threshold_hit: bool) -> &'static str {
    match (sample_kind, threshold_hit) {
        (MemorySampleKind::Lifecycle, true) => "lifecycle+threshold",
        (MemorySampleKind::Lifecycle, false) => "lifecycle",
        (MemorySampleKind::Periodic, true) => "periodic+threshold",
        (MemorySampleKind::Periodic, false) => "periodic",
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PsMemoryKb {
    pub(crate) rss_kib: u64,
    pub(crate) vsz_kib: u64,
}

#[cfg(any(target_os = "macos", test))]
impl PsMemoryKb {
    fn into_sample(self, sample_ms: u128) -> ProcessMemorySample {
        ProcessMemorySample {
            rss_bytes: self.rss_kib.saturating_mul(1024),
            vsz_bytes: self.vsz_kib.saturating_mul(1024),
            sample_ms,
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_ps_memory_output(output: &str) -> Option<PsMemoryKb> {
    let mut nums = output
        .split_whitespace()
        .filter_map(|part| part.parse::<u64>().ok());
    let rss_kib = nums.next()?;
    let vsz_kib = nums.next()?;
    Some(PsMemoryKb { rss_kib, vsz_kib })
}

fn sample_current_process_memory() -> Result<ProcessMemorySample, ()> {
    #[cfg(target_os = "macos")]
    {
        let started = std::time::Instant::now();
        let output = std::process::Command::new("/bin/ps")
            .args([
                "-o",
                "rss=",
                "-o",
                "vsz=",
                "-p",
                &std::process::id().to_string(),
            ])
            .output()
            .map_err(|_| ())?;
        if !output.status.success() {
            return Err(());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_ps_memory_output(&stdout)
            .map(|kb| kb.into_sample(started.elapsed().as_millis()))
            .ok_or(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(())
    }
}

pub(crate) fn retained_memory_stats(
    state: &AppState,
    history_replay_cache_bytes: usize,
) -> RetainedMemoryStats {
    let markdown_memo_cache_bytes =
        crate::transcript::render::assistant::committed_markdown_memo_estimated_bytes();
    RetainedMemoryStats {
        message_history_payload_bytes: estimate_unique_message_payload_bytes(state),
        transcript_cell_text_bytes: estimate_transcript_cell_text_bytes(state),
        tool_execution_bytes: estimate_tool_execution_bytes(state),
        reasoning_metadata_bytes: state
            .session
            .reasoning_metadata
            .len()
            .saturating_mul(std::mem::size_of::<crate::state::session::ReasoningMetadata>()),
        subagent_bytes: estimate_subagent_bytes(state),
        last_markdown_bytes: state
            .session
            .last_agent_markdown
            .as_ref()
            .map_or(0, String::len),
        markdown_memo_cache_bytes,
        history_replay_cache_bytes,
    }
}

fn estimate_unique_message_payload_bytes(state: &AppState) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut total = 0usize;
    for cell in state.session.transcript.cells() {
        if seen.insert(cell.message_uuid)
            && let Ok(bytes) = serde_json::to_vec(cell.source.as_ref())
        {
            total = total.saturating_add(bytes.len());
        }
    }
    for message in &state.session.rewind_pre_clear_messages {
        if let Some(uuid) = message.uuid()
            && !seen.insert(*uuid)
        {
            continue;
        }
        if let Ok(bytes) = serde_json::to_vec(message.as_ref()) {
            total = total.saturating_add(bytes.len());
        }
    }
    total
}

fn estimate_transcript_cell_text_bytes(state: &AppState) -> usize {
    let cell_bytes = state
        .session
        .transcript
        .cells()
        .iter()
        .map(|cell| match &cell.kind {
            CellKind::UserText { text }
            | CellKind::AssistantText { text, .. }
            | CellKind::AssistantThinking { text, .. } => text.len(),
            CellKind::ToolUse { call_id, tool_name } => {
                call_id.len().saturating_add(tool_name.len())
            }
            CellKind::ToolResult { call_id } => call_id.len(),
            CellKind::AssistantRedactedThinking { .. }
            | CellKind::Attachment
            | CellKind::System(_) => 0,
        })
        .fold(0usize, usize::saturating_add);
    let streaming_bytes = state.ui.streaming.as_ref().map_or(0usize, |streaming| {
        streaming
            .content
            .len()
            .saturating_add(streaming.thinking.len())
    });
    cell_bytes.saturating_add(streaming_bytes)
}

fn estimate_tool_execution_bytes(state: &AppState) -> usize {
    let executions = state
        .session
        .tool_executions
        .iter()
        .map(|tool| {
            tool.call_id
                .len()
                .saturating_add(tool.name.len())
                .saturating_add(tool.description.as_ref().map_or(0, String::len))
                .saturating_add(tool.input_preview.as_ref().map_or(0, String::len))
                .saturating_add(tool.streaming_input.as_ref().map_or(0, String::len))
        })
        .fold(0usize, usize::saturating_add);
    let summaries = state
        .session
        .tool_group_summaries
        .iter()
        .map(|(key, value)| key.len().saturating_add(value.len()))
        .fold(0usize, usize::saturating_add);
    executions.saturating_add(summaries)
}

fn estimate_subagent_bytes(state: &AppState) -> usize {
    let subagents = state
        .session
        .subagents
        .iter()
        .map(|agent| {
            agent
                .agent_id
                .len()
                .saturating_add(agent.agent_type.len())
                .saturating_add(agent.description.len())
                .saturating_add(agent.team_name.as_ref().map_or(0, String::len))
                .saturating_add(agent.last_tool_name.as_ref().map_or(0, String::len))
                .saturating_add(agent.final_message.as_ref().map_or(0, String::len))
                .saturating_add(
                    agent
                        .recent_activities
                        .iter()
                        .filter_map(|activity| serde_json::to_vec(activity).ok())
                        .map(|bytes| bytes.len())
                        .sum::<usize>(),
                )
        })
        .fold(0usize, usize::saturating_add);
    let summaries = state
        .session
        .subagent_summaries
        .iter()
        .map(|(tool_use_id, summary)| tool_use_id.len().saturating_add(summary.agent_type.len()))
        .fold(0usize, usize::saturating_add);
    let tasks = state
        .session
        .active_tasks
        .iter()
        .map(|task| {
            task.task_id
                .len()
                .saturating_add(task.description.len())
                .saturating_add(task.workflow_name.as_ref().map_or(0, String::len))
                .saturating_add(
                    task.workflow_progress
                        .iter()
                        .filter_map(|event| serde_json::to_vec(event).ok())
                        .map(|bytes| bytes.len())
                        .sum::<usize>(),
                )
        })
        .fold(0usize, usize::saturating_add);
    subagents.saturating_add(summaries).saturating_add(tasks)
}

pub(crate) fn memory_phase_for_core_event(event: &CoreEvent) -> Option<MemoryPhase> {
    let CoreEvent::Protocol(notification) = event else {
        return None;
    };
    match notification {
        coco_types::ServerNotification::TurnStarted(_) => Some(MemoryPhase::TurnStarted),
        coco_types::ServerNotification::StreamRequestEnd { .. } => {
            Some(MemoryPhase::EngineReturned)
        }
        coco_types::ServerNotification::TurnEnded(_) => Some(MemoryPhase::TurnEnded),
        coco_types::ServerNotification::HistoryReplaced { .. } => {
            Some(MemoryPhase::HistoryReplaced)
        }
        coco_types::ServerNotification::ContextCleared(_) => Some(MemoryPhase::ContextCleared),
        coco_types::ServerNotification::MessageTruncated { .. } => {
            Some(MemoryPhase::MessageTruncated)
        }
        coco_types::ServerNotification::SessionResetForResume { .. } => {
            Some(MemoryPhase::SessionReset)
        }
        _ => None,
    }
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn parses_macos_ps_memory_output_and_converts_kib_to_bytes() {
        let parsed = parse_ps_memory_output("  12345  67890\n").expect("parse ps output");
        let sample = parsed.into_sample(7);
        assert_eq!(sample.rss_bytes, 12_641_280);
        assert_eq!(sample.vsz_bytes, 69_519_360);
        assert_eq!(sample.sample_ms, 7);
    }

    #[test]
    fn parses_ps_output_with_header_or_extra_whitespace() {
        let parsed = parse_ps_memory_output("RSS VSZ\n 42\t9001\n").expect("parse ps output");
        assert_eq!(parsed.rss_kib, 42);
        assert_eq!(parsed.vsz_kib, 9001);
    }

    #[test]
    fn memory_periodic_and_threshold_can_be_disabled_independently() {
        let mut config = TuiPerformanceConfig {
            enabled: false,
            sample_every_n_frames: 10,
            slow_frame_ms: 16,
            slow_stage_us: 1000,
            memory_enabled: true,
            memory_sample_interval_secs: 0,
            memory_delta_threshold_bytes: 0,
        };
        assert!(MemoryPerfTracker::enabled(config));
        assert!(!MemoryPerfTracker::periodic_enabled(config));

        config.memory_sample_interval_secs = 30;
        assert!(MemoryPerfTracker::periodic_enabled(config));
        assert_eq!(
            MemoryPerfTracker::periodic_interval(config),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn trigger_labels_merge_threshold_with_single_sample() {
        assert_eq!(
            trigger_label(MemorySampleKind::Lifecycle, true),
            "lifecycle+threshold"
        );
        assert_eq!(
            trigger_label(MemorySampleKind::Periodic, true),
            "periodic+threshold"
        );
        assert_eq!(
            trigger_label(MemorySampleKind::Lifecycle, false),
            "lifecycle"
        );
        assert_eq!(trigger_label(MemorySampleKind::Periodic, false), "periodic");
    }

    #[test]
    fn retained_stats_follow_transcript_append_truncate_reset() {
        let mut state = AppState::new();
        state
            .session
            .transcript
            .on_message_appended(std::sync::Arc::new(
                coco_messages::create_user_message_with_uuid(uuid::Uuid::new_v4(), "hello"),
            ));
        let after_append = retained_memory_stats(&state, 123);
        assert!(after_append.message_history_payload_bytes > 0);
        assert!(after_append.transcript_cell_text_bytes >= "hello".len());
        assert_eq!(after_append.history_replay_cache_bytes, 123);

        state.session.transcript.on_message_truncated(0);
        let after_truncate = retained_memory_stats(&state, 0);
        assert_eq!(after_truncate.transcript_cell_text_bytes, 0);
        assert_eq!(after_truncate.history_replay_cache_bytes, 0);

        state.session.last_agent_markdown = Some("cached markdown".to_string());
        state.session.transcript.on_session_reset();
        state.session.last_agent_markdown = None;
        let after_reset = retained_memory_stats(&state, 0);
        assert_eq!(after_reset.message_history_payload_bytes, 0);
        assert_eq!(after_reset.last_markdown_bytes, 0);
    }
}
