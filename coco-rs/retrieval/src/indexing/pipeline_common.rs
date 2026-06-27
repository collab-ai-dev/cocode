//! Common types and functions shared between IndexPipeline and TagPipeline.
//!
//! This module provides generic pipeline infrastructure to reduce code duplication
//! while keeping each pipeline's implementation simple and focused.

use super::BatchId;
use super::LagInfo;

/// Strict mode configuration for pipelines.
///
/// Controls whether the pipeline should wait for all events to complete
/// before reporting ready status.
#[derive(Debug, Clone)]
pub struct StrictModeConfig {
    /// Initial build must complete before Ready.
    pub init: bool,
    /// Incremental updates must complete before Ready.
    pub incremental: bool,
}

impl Default for StrictModeConfig {
    fn default() -> Self {
        Self {
            init: true,
            incremental: false,
        }
    }
}

/// Generic pipeline state.
///
/// Parameterized by the stats type `S` to support different pipeline statistics.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineState<S> {
    /// Pipeline has not been initialized yet.
    Uninitialized,
    /// Pipeline is building.
    Building {
        /// Current batch ID.
        batch_id: BatchId,
        /// Progress percentage (0.0 - 1.0).
        progress: f32,
        /// Unix timestamp when building started.
        started_at: i64,
    },
    /// Pipeline is ready.
    Ready {
        /// Pipeline statistics.
        stats: S,
        /// Unix timestamp when completed.
        completed_at: i64,
    },
    /// Pipeline failed to initialize.
    Failed {
        /// Error message.
        error: String,
        /// Unix timestamp when failure occurred.
        failed_at: i64,
    },
}

/// Generic readiness status for queries.
///
/// Parameterized by the stats type `S` to support different pipeline statistics.
#[derive(Debug, Clone)]
pub enum PipelineReadiness<S> {
    /// Pipeline not initialized.
    Uninitialized,
    /// Pipeline is building.
    Building {
        /// Progress percentage.
        progress: f32,
        /// Current lag info.
        lag_info: LagInfo,
    },
    /// Pipeline is ready.
    Ready {
        /// Pipeline statistics.
        stats: S,
        /// Current lag info.
        lag_info: LagInfo,
    },
    /// Pipeline is not ready (strict mode).
    NotReady {
        /// Reason for not being ready.
        reason: String,
        /// Current lag info.
        lag_info: Option<LagInfo>,
        /// Whether partial results are available.
        is_partial_available: bool,
    },
    /// Pipeline failed.
    Failed {
        /// Error message.
        error: String,
    },
}

/// Compute readiness status from pipeline state.
///
/// This function encapsulates the common logic for determining whether a pipeline
/// is ready to serve queries, taking into account strict mode configuration and
/// pending events (lag).
pub fn compute_readiness<S: Clone>(
    state: &PipelineState<S>,
    lag_info: LagInfo,
    init_complete: bool,
    strict_config: &StrictModeConfig,
) -> PipelineReadiness<S> {
    match state {
        PipelineState::Uninitialized => PipelineReadiness::Uninitialized,

        PipelineState::Building { progress, .. } => PipelineReadiness::Building {
            progress: *progress,
            lag_info,
        },

        PipelineState::Ready { stats, .. } => {
            if lag_info.lag > 0 {
                // There are pending events
                let is_strict = if !init_complete {
                    strict_config.init
                } else {
                    strict_config.incremental
                };

                if is_strict {
                    PipelineReadiness::NotReady {
                        reason: format!(
                            "{} mode: {} events pending",
                            if init_complete { "Incremental" } else { "Init" },
                            lag_info.lag
                        ),
                        lag_info: Some(lag_info),
                        is_partial_available: true,
                    }
                } else {
                    PipelineReadiness::Ready {
                        stats: stats.clone(),
                        lag_info,
                    }
                }
            } else {
                PipelineReadiness::Ready {
                    stats: stats.clone(),
                    lag_info,
                }
            }
        }

        PipelineState::Failed { error, .. } => PipelineReadiness::Failed {
            error: error.clone(),
        },
    }
}

/// Get current Unix timestamp.
#[inline]
pub fn now_timestamp() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
#[path = "pipeline_common.test.rs"]
mod tests;
