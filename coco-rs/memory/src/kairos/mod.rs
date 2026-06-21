//! KAIROS assistant-mode features.
//!
//! Pure path resolution and append helpers for the daily log file.
//! Auto-trigger / scheduler logic lives in [`crate::service::dream`].
//!
//! Scope of this module: pure path resolution + append helpers for
//! the daily log file. Auto-trigger / scheduler logic lives in
//! [`crate::service::dream`].

pub mod daily_log;
pub mod rollover;

pub use daily_log::DailyLogStore;
pub use daily_log::daily_log_path;
pub use rollover::KairosRolloverWatcher;
