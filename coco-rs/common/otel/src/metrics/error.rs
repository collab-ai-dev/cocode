use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, MetricsError>;

#[derive(Debug, Error)]
pub enum MetricsError {
    // Metrics.
    #[error("metric name cannot be empty")]
    EmptyMetricName,
    #[error("metric name contains invalid characters: {name}")]
    InvalidMetricName { name: String },
    #[error("{label} cannot be empty")]
    EmptyTagComponent { label: String },
    #[error("{label} contains invalid characters: {value}")]
    InvalidTagComponent { label: String, value: String },

    #[error("metrics exporter is disabled")]
    ExporterDisabled,

    #[error("counter increment must be non-negative for {name}: {inc}")]
    NegativeCounterIncrement { name: String, inc: i64 },

    #[error("failed to build OTLP metrics exporter")]
    ExporterBuild {
        #[source]
        source: opentelemetry_otlp::ExporterBuildError,
    },

    #[error("invalid OTLP metrics configuration: {message}")]
    InvalidConfig { message: String },

    #[error("failed to flush or shutdown metrics provider")]
    ProviderShutdown {
        #[source]
        source: opentelemetry_sdk::error::OTelSdkError,
    },
}

// Layer the `coco-error` traits on top of the existing `thiserror` enum so
// callers can pivot on `StatusCode`. The `next()` impl returns `None` because
// the wrapped sources (`opentelemetry_otlp` / `opentelemetry_sdk`) don't
// implement `StackError` — `Display` already includes their message via
// `thiserror`'s `#[source]` chain.
impl StackError for MetricsError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for MetricsError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::EmptyMetricName
            | Self::InvalidMetricName { .. }
            | Self::EmptyTagComponent { .. }
            | Self::InvalidTagComponent { .. }
            | Self::NegativeCounterIncrement { .. } => StatusCode::InvalidArguments,
            Self::ExporterDisabled => StatusCode::Unsupported,
            Self::ExporterBuild { .. } => StatusCode::ServiceUnavailable,
            Self::InvalidConfig { .. } => StatusCode::InvalidConfig,
            Self::ProviderShutdown { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
