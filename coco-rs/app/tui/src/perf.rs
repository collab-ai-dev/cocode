use std::time::Duration;

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;

use crate::display_settings::TuiPerformanceConfig;

pub(crate) const TARGET: &str = "tui::perf::frame";

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
