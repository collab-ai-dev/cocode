use std::time::Duration;

pub(crate) const APP_SERVER_LOCAL_CHANNEL_CAPACITY: usize = 128;
pub(crate) const APP_SERVER_LOCAL_RETENTION_PER_SESSION: usize = 128;
pub const APP_SERVER_TURN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn server_config_usize(value: i64, fallback: usize) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn server_config_duration_secs(value: i64, fallback: Duration) -> Duration {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}
