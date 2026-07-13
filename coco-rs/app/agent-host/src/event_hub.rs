use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use coco_hub_connector::HubConnectorQueueError;
use coco_hub_connector::HubConnectorSender;
use coco_hub_connector::HubConnectorWorker;
use coco_hub_connector::HubConnectorWorkerConfig;
use coco_hub_connector::protocol::AnnounceFrame;
use coco_types::{SessionEnvelope, SessionId};
use uuid::Uuid;

use crate::BUILD_PACKAGE_VERSION;
use crate::app_session::AppSessionHandle;
use crate::shutdown::ShutdownDrainOutcome;

const CHANNEL_CAPACITY: usize = 1024;
const RING_CAPACITY: usize = 10_000;
const BATCH_MAX_EVENTS: usize = 1_000;
const BATCH_MAX_BYTES: usize = 1_048_576;
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(100);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

pub struct ProcessEventHub {
    worker: HubConnectorWorker,
    updater: ProcessEventHubUpdater,
}

#[derive(Clone)]
pub struct ProcessEventHubEgress {
    sender: HubConnectorSender,
    updater: ProcessEventHubUpdater,
}

#[derive(Clone)]
pub struct ProcessEventHubUpdater {
    sender: HubConnectorSender,
    cwd: PathBuf,
}

impl ProcessEventHub {
    pub fn spawn(
        runtime_config: &coco_config::RuntimeConfig,
        cwd: &Path,
        live_sessions: Vec<SessionId>,
    ) -> Option<Self> {
        let url = runtime_config.event_hub.url.clone()?;
        match HubConnectorWorker::spawn(HubConnectorWorkerConfig {
            url,
            announce: announce_frame(live_sessions, cwd),
            channel_capacity: CHANNEL_CAPACITY,
            pending_capacity: RING_CAPACITY,
            batch_max_events: BATCH_MAX_EVENTS,
            batch_max_bytes: BATCH_MAX_BYTES,
            flush_interval: FLUSH_INTERVAL,
            reconnect_initial_delay: RECONNECT_INITIAL_DELAY,
            reconnect_max_delay: RECONNECT_MAX_DELAY,
        }) {
            Ok(worker) => {
                let updater = ProcessEventHubUpdater {
                    sender: worker.sender(),
                    cwd: cwd.to_path_buf(),
                };
                Some(Self { worker, updater })
            }
            Err(error) => {
                tracing::warn!(%error, "event hub connector worker failed to start");
                None
            }
        }
    }

    pub fn egress(&self) -> ProcessEventHubEgress {
        ProcessEventHubEgress {
            sender: self.worker.sender(),
            updater: self.updater.clone(),
        }
    }

    pub fn updater(&self) -> ProcessEventHubUpdater {
        self.updater.clone()
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

pub fn spawn_app_server_membership_watcher(
    app_server: Arc<coco_app_server::AppServer<AppSessionHandle>>,
    updater: ProcessEventHubUpdater,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut revisions = app_server.subscribe_session_activity();
        let mut last_live_sessions = app_server_live_session_ids(&app_server);
        if !last_live_sessions.is_empty() {
            updater
                .update_live_sessions(last_live_sessions.clone())
                .await;
        }
        while revisions.changed().await.is_ok() {
            let next_live_sessions = app_server_live_session_ids(&app_server);
            if next_live_sessions == last_live_sessions {
                continue;
            }
            updater
                .update_live_sessions(next_live_sessions.clone())
                .await;
            last_live_sessions = next_live_sessions;
        }
    })
}

impl ProcessEventHubUpdater {
    pub async fn update_live_sessions(&self, live_sessions: Vec<SessionId>) {
        if let Err(error) = self
            .sender
            .update_announce(announce_frame(live_sessions, &self.cwd))
            .await
        {
            tracing::warn!(%error, "failed to update event hub live-session membership");
        }
    }
}

impl ProcessEventHubEgress {
    pub async fn update_live_sessions(&self, live_sessions: Vec<SessionId>) {
        self.updater.update_live_sessions(live_sessions).await;
    }

    pub async fn sync_app_server_membership_if_changed<H>(
        &self,
        app_server: &coco_app_server::AppServer<H>,
        last_live_sessions: &mut Vec<SessionId>,
    ) where
        H: Clone + Send + Sync + 'static,
    {
        let live_sessions = app_server_live_session_ids(app_server);
        if live_sessions == *last_live_sessions {
            return;
        }
        self.update_live_sessions(live_sessions.clone()).await;
        *last_live_sessions = live_sessions;
    }

    pub fn try_enqueue(&self, envelope: SessionEnvelope) -> Result<(), HubConnectorQueueError> {
        self.sender.try_enqueue(envelope)
    }
}

pub fn app_server_live_session_ids<H>(
    app_server: &coco_app_server::AppServer<H>,
) -> Vec<coco_types::SessionId>
where
    H: Clone + Send + Sync + 'static,
{
    app_server
        .list_live_sessions()
        .into_iter()
        .map(|summary| summary.session_id)
        .collect()
}

fn announce_frame(live_sessions: Vec<SessionId>, cwd: &Path) -> AnnounceFrame {
    AnnounceFrame {
        instance_id: persisted_instance_id(),
        live_sessions,
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

#[cfg(test)]
#[path = "event_hub.test.rs"]
mod tests;
