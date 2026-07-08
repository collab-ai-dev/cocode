//! Simplified Event Hub server backed directly by local session JSONL files.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

mod display;
pub mod local_store;
pub mod routes;
pub mod sqlite_store;
pub mod store;

pub use local_store::LocalSessionJsonStore;
pub use routes::AppState;
pub use routes::router;
pub use sqlite_store::SqliteEventStore;
pub use store::EventRow;
pub use store::EventStore;
pub use store::EventStoreError;
pub use store::InstanceRow;
pub use store::RetentionPolicy;
pub use store::SearchQuery;
pub use store::SessionRow;

pub type HubServerResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct SqliteHubServerOptions {
    pub data_dir: PathBuf,
    pub retention_policy: RetentionPolicy,
    pub retention_sweep_interval_secs: u64,
}

impl SqliteHubServerOptions {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            retention_policy: RetentionPolicy {
                retention_days: 3,
                retention_max_bytes: 3_221_225_472,
            },
            retention_sweep_interval_secs: 900,
        }
    }
}

pub async fn serve_sqlite_until(
    addr: SocketAddr,
    options: SqliteHubServerOptions,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> HubServerResult<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_sqlite_listener_until(listener, options, shutdown).await
}

pub async fn serve_sqlite_listener_until(
    listener: tokio::net::TcpListener,
    options: SqliteHubServerOptions,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> HubServerResult<()> {
    let db_path = options.data_dir.join("events.sqlite");
    let store = SqliteEventStore::open(db_path)?;
    spawn_retention_task(
        store.clone(),
        options.retention_policy,
        options.retention_sweep_interval_secs,
    );
    let app = router(AppState::new(store));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

pub fn spawn_retention_task(store: SqliteEventStore, policy: RetentionPolicy, interval_secs: u64) {
    let interval_secs = interval_secs.max(1);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            match store.run_retention_sweep(&policy).await {
                Ok(stats) => {
                    tracing::debug!(
                        deleted_events = stats.deleted_events,
                        deleted_sessions = stats.deleted_sessions,
                        freed_bytes = stats.freed_bytes,
                        "hub retention sweep completed"
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "hub retention sweep failed");
                }
            }
        }
    });
}
