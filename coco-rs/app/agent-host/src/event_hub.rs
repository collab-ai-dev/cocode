use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use coco_hub_connector::HubConnectorSender;
use coco_hub_connector::HubConnectorWorker;
use coco_hub_connector::HubConnectorWorkerConfig;
use coco_hub_connector::protocol::AnnounceFrame;
use coco_types::SessionId;
use uuid::Uuid;

use crate::BUILD_PACKAGE_VERSION;
use crate::app_server_host::AppServerLocalBridge;
use crate::session_runtime::SessionHandle;
use crate::shutdown::ShutdownDrainOutcome;

const CHANNEL_CAPACITY: usize = 1024;
const RING_CAPACITY: usize = 10_000;
const BATCH_MAX_EVENTS: usize = 1_000;
const BATCH_MAX_BYTES: usize = 1_048_576;
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(100);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

pub struct RuntimeEventHubConnector {
    worker: HubConnectorWorker,
}

impl RuntimeEventHubConnector {
    pub fn spawn_and_attach_for_session(
        bridge: &AppServerLocalBridge,
        session: &SessionHandle,
        cwd: &Path,
    ) -> Option<Self> {
        let connector =
            Self::spawn_for_session(session.runtime_config(), session.session_id().clone(), cwd);
        if let Some(connector) = &connector {
            bridge.set_hub_connector_sender(connector.sender());
        }
        connector
    }

    pub fn spawn_for_session(
        runtime_config: &coco_config::RuntimeConfig,
        session_id: SessionId,
        cwd: &Path,
    ) -> Option<Self> {
        let url = runtime_config.event_hub.url.clone()?;
        match HubConnectorWorker::spawn(HubConnectorWorkerConfig {
            url,
            announce: announce_frame(session_id, cwd),
            channel_capacity: CHANNEL_CAPACITY,
            pending_capacity: RING_CAPACITY,
            batch_max_events: BATCH_MAX_EVENTS,
            batch_max_bytes: BATCH_MAX_BYTES,
            flush_interval: FLUSH_INTERVAL,
            reconnect_initial_delay: RECONNECT_INITIAL_DELAY,
            reconnect_max_delay: RECONNECT_MAX_DELAY,
        }) {
            Ok(worker) => Some(Self { worker }),
            Err(error) => {
                tracing::warn!(%error, "event hub connector worker failed to start");
                None
            }
        }
    }

    pub fn sender(&self) -> HubConnectorSender {
        self.worker.sender()
    }

    pub async fn shutdown_and_flush_with_timeout(self, timeout: Duration) -> ShutdownDrainOutcome {
        tokio::select! {
            result = tokio::time::timeout(timeout, self.worker.shutdown_and_flush()) => {
                match result {
                    Ok(Ok(stats)) => {
                        tracing::info!(
                            shipped_events = stats.shipped_events,
                            skipped_ephemeral_events = stats.skipped_ephemeral_events,
                            dropped_durable_events = stats.dropped_durable_events,
                            "event hub connector flushed"
                        );
                        ShutdownDrainOutcome::Clean
                    }
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        tracing::warn!(error = %message, "event hub connector shutdown flush failed");
                        ShutdownDrainOutcome::Failed { message }
                    }
                    Err(_) => {
                        let timeout_secs = timeout.as_secs();
                        tracing::warn!(
                            timeout_secs,
                            "event hub connector shutdown flush timed out"
                        );
                        ShutdownDrainOutcome::TimedOut { timeout_secs }
                    }
                }
            }
            () = crate::shutdown::os_interrupt_signal() => {
                tracing::warn!("event hub connector shutdown flush interrupted by signal");
                ShutdownDrainOutcome::Interrupted
            }
        }
    }
}

fn announce_frame(session_id: SessionId, cwd: &Path) -> AnnounceFrame {
    AnnounceFrame {
        instance_id: persisted_instance_id(),
        live_sessions: vec![session_id],
        host: std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string()),
        cwd: cwd.display().to_string(),
        pid: i64::from(std::process::id()),
        started_at: Utc::now(),
        version: BUILD_PACKAGE_VERSION.to_string(),
        instance_kind: "interactive".to_string(),
        entrypoint: Some("cocode".to_string()),
        name: None,
    }
}

/// Load-or-create this install's stable Hub instance id. Hub
/// cursors are ` (instance_id, session_id)`-scoped, so a per-start random id
/// fragments one session's history across phantom instances and silently
/// voids cross-restart resume cursors. Persisted under `<config_home>/instance-id`
/// so restarts of the same install reuse it. (Concurrent processes on one
/// install share the id; that is harmless because sessions never span
/// processes, so ` (instance, session)` keys stay unique.)
fn persisted_instance_id() -> Uuid {
    let path = coco_config::global_config::config_home().join("instance-id");
    if let Ok(contents) = std::fs::read_to_string(&path)
        && let Ok(id) = Uuid::parse_str(contents.trim())
    {
        return id;
    }
    let id = Uuid::new_v4();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(&path, id.to_string()) {
        tracing::warn!(%error, "failed to persist hub instance id; using an ephemeral id this run");
    }
    id
}
