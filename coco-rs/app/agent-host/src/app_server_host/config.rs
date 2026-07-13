use std::time::Duration;

use coco_app_server::SurfaceLimits;

pub(crate) const APP_SERVER_LOCAL_CHANNEL_CAPACITY: usize = 128;
pub(crate) const APP_SERVER_LOCAL_RETENTION_PER_SESSION: usize = 128;
pub(crate) const APP_SERVER_MAX_SURFACES_PER_CONNECTION: usize = 8;
pub(crate) const APP_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION: usize = 16;
pub const APP_SERVER_TURN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn server_config_usize(value: i64, fallback: usize) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn server_config_surface_limits(
    server_config: &coco_config::ServerConfig,
) -> SurfaceLimits {
    SurfaceLimits {
        max_surfaces_per_connection: server_config_usize(
            server_config.max_surfaces_per_connection,
            APP_SERVER_MAX_SURFACES_PER_CONNECTION,
        ),
        max_passive_surfaces_per_session: server_config_usize(
            server_config.max_passive_surfaces_per_session,
            APP_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION,
        ),
    }
}

pub(crate) fn server_config_duration_secs(value: i64, fallback: Duration) -> Duration {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}
